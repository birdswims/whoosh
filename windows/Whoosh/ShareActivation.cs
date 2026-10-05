using Microsoft.UI.Dispatching;
using Microsoft.Windows.AppLifecycle;
using Windows.ApplicationModel.Activation;
using Windows.ApplicationModel.DataTransfer;
using Windows.ApplicationModel.DataTransfer.ShareTarget;
using Windows.Storage;
using Windows.Storage.Streams;

namespace Whoosh;

internal sealed class ShareHandoff
{
    readonly ShareOperation _operation;
    bool _reported;

    public ShareHandoff(ShareOperation operation, IReadOnlyList<string> paths)
    {
        _operation = operation;
        Paths = paths;
    }

    public IReadOnlyList<string> Paths { get; }

    public void Complete()
    {
        if (_reported)
        {
            return;
        }

        _reported = true;
        try
        {
            _operation.ReportCompleted();
        }
        catch
        {
            // The share window already closed.
        }
    }

    public void Fail(string message)
    {
        if (_reported)
        {
            return;
        }

        _reported = true;
        try
        {
            _operation.ReportError(message);
        }
        catch
        {
            // The share window already closed.
        }
    }
}

internal static class ShareActivation
{
    public static ShareHandoff? Take()
    {
        AppActivationArguments activated;
        try
        {
            activated = AppInstance.GetCurrent().GetActivatedEventArgs();
        }
        catch
        {
            return null;
        }

        if (activated.Kind != ExtendedActivationKind.ShareTarget
            || activated.Data is not ShareTargetActivatedEventArgs share)
        {
            return null;
        }

        return Collect(share);
    }

    static ShareHandoff Collect(ShareTargetActivatedEventArgs share)
    {
        var done = new TaskCompletionSource<ShareHandoff>(TaskCreationOptions.RunContinuationsAsynchronously);
        var controller = DispatcherQueueController.CreateOnDedicatedThread();
        var queued = controller.DispatcherQueue.TryEnqueue(async () =>
        {
            try
            {
                done.TrySetResult(await Read(share.ShareOperation));
            }
            catch (Exception ex)
            {
                done.TrySetException(ex);
            }
        });
        if (!queued)
        {
            try
            {
                share.ShareOperation.ReportError("Whoosh could not read that item.");
            }
            catch
            {
                // The share window already closed.
            }

            throw new InvalidOperationException("Whoosh could not read that item.");
        }

        try
        {
            return done.Task.GetAwaiter().GetResult();
        }
        finally
        {
            _ = controller.ShutdownQueueAsync();
        }
    }

    static async Task<ShareHandoff> Read(ShareOperation operation)
    {
        operation.ReportStarted();
        try
        {
            var paths = await Gather(operation.Data);
            if (paths.Count == 0)
            {
                operation.ReportError("Whoosh could not read that item.");
                throw new InvalidOperationException("Whoosh could not read that item.");
            }

            return new ShareHandoff(operation, paths);
        }
        catch (InvalidOperationException)
        {
            throw;
        }
        catch
        {
            try
            {
                operation.ReportError("Whoosh could not read that item.");
            }
            catch
            {
                // The share window already closed.
            }

            throw;
        }
    }

    static async Task<List<string>> Gather(DataPackageView data)
    {
        var paths = new List<string>();
        string? inbox = null;

        if (data.Contains(StandardDataFormats.StorageItems))
        {
            var items = await data.GetStorageItemsAsync();
            foreach (var item in items)
            {
                if (item.IsOfType(StorageItemTypes.Folder))
                {
                    if (!string.IsNullOrEmpty(item.Path) && Directory.Exists(item.Path))
                    {
                        paths.Add(item.Path);
                    }

                    continue;
                }

                if (!string.IsNullOrEmpty(item.Path) && File.Exists(item.Path))
                {
                    paths.Add(item.Path);
                    continue;
                }

                if (item is StorageFile file)
                {
                    inbox ??= NewInbox();
                    var destination = Unique(inbox, string.IsNullOrEmpty(file.Name) ? "shared" : file.Name);
                    using var stream = await file.OpenReadAsync();
                    await CopyStream(stream.GetInputStreamAt(0), destination);
                    paths.Add(destination);
                }
            }
        }

        if (paths.Count == 0 && data.Contains(StandardDataFormats.Bitmap))
        {
            inbox ??= NewInbox();
            var reference = await data.GetBitmapAsync();
            using var stream = await reference.OpenReadAsync();
            var raw = Unique(inbox, "image.bin");
            await CopyStream(stream.GetInputStreamAt(0), raw);
            var imageName = ImageName(raw);
            var named = raw;
            if (!string.Equals(imageName, "image.bin", StringComparison.OrdinalIgnoreCase))
            {
                named = Unique(inbox, imageName);
                File.Move(raw, named);
            }

            paths.Add(named);
        }

        if (paths.Count == 0 && data.Contains(StandardDataFormats.Text))
        {
            var text = await data.GetTextAsync();
            if (!string.IsNullOrWhiteSpace(text))
            {
                inbox ??= NewInbox();
                var destination = Unique(inbox, "shared.txt");
                await File.WriteAllTextAsync(destination, text);
                paths.Add(destination);
            }
        }

        if (paths.Count == 0 && data.Contains(StandardDataFormats.Uri))
        {
            var uri = await data.GetUriAsync();
            if (uri is not null)
            {
                inbox ??= NewInbox();
                var destination = Unique(inbox, "link.txt");
                await File.WriteAllTextAsync(destination, uri.AbsoluteUri);
                paths.Add(destination);
            }
        }

        if (paths.Count == 0 && data.Contains(StandardDataFormats.WebLink))
        {
            var uri = await data.GetWebLinkAsync();
            if (uri is not null)
            {
                inbox ??= NewInbox();
                var destination = Unique(inbox, "link.txt");
                await File.WriteAllTextAsync(destination, uri.AbsoluteUri);
                paths.Add(destination);
            }
        }

        return paths;
    }

    static async Task CopyStream(IInputStream input, string destination)
    {
        using var reader = new DataReader(input);
        reader.InputStreamOptions = InputStreamOptions.Partial;
        await using var output = File.Create(destination);
        while (true)
        {
            var loaded = await reader.LoadAsync(64 * 1024);
            if (loaded == 0)
            {
                break;
            }

            var bytes = new byte[loaded];
            reader.ReadBytes(bytes);
            await output.WriteAsync(bytes);
        }
    }

    static string NewInbox()
    {
        var inbox = Path.Combine(Path.GetTempPath(), "Whoosh", "share", Guid.NewGuid().ToString("n"));
        Directory.CreateDirectory(inbox);
        return inbox;
    }

    static string Unique(string directory, string name)
    {
        var cleaned = string.Join("_", name.Split(Path.GetInvalidFileNameChars(), StringSplitOptions.RemoveEmptyEntries));
        if (string.IsNullOrWhiteSpace(cleaned))
        {
            cleaned = "shared";
        }

        var destination = Path.Combine(directory, cleaned);
        if (!File.Exists(destination) && !Directory.Exists(destination))
        {
            return destination;
        }

        var stem = Path.GetFileNameWithoutExtension(cleaned);
        var extension = Path.GetExtension(cleaned);
        for (var index = 2; index < 1000; index++)
        {
            destination = Path.Combine(directory, $"{stem} {index}{extension}");
            if (!File.Exists(destination))
            {
                return destination;
            }
        }

        return Path.Combine(directory, Guid.NewGuid().ToString("n") + extension);
    }

    static string ImageName(string path)
    {
        var header = new byte[8];
        using var stream = File.OpenRead(path);
        var read = stream.Read(header, 0, header.Length);
        if (read >= 4 && header[0] == 0x89 && header[1] == 0x50)
        {
            return "image.png";
        }

        if (read >= 3 && header[0] == 0xFF && header[1] == 0xD8)
        {
            return "image.jpg";
        }

        if (read >= 2 && header[0] == (byte)'B' && header[1] == (byte)'M')
        {
            return "image.bmp";
        }

        if (read >= 3 && header[0] == (byte)'G' && header[1] == (byte)'I')
        {
            return "image.gif";
        }

        return "image.bin";
    }
}
