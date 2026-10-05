using System.Runtime.InteropServices.WindowsRuntime;
using Microsoft.UI.Dispatching;
using Windows.ApplicationModel.DataTransfer;
using Windows.Graphics.Imaging;
using Windows.Storage.Streams;

namespace Whoosh;

/// <summary>
/// Watches the Windows clipboard and applies a snapshot from a trusted device.
/// The engine keeps that snapshot in memory and serves it only to devices this computer has trusted.
/// </summary>
static class ClipboardSync
{
    const int MaxTextBytes = 1024 * 1024;
    const int MaxImageBytes = 8 * 1024 * 1024;

    static Action<string, byte[]?>? _publish;
    static DispatcherQueue? _queue;
    static int _generation;
    static long _suppressUntil;
    static bool _watching;

    public static void Watch(DispatcherQueue queue, Action<string, byte[]?> publish)
    {
        _queue = queue;
        _publish = publish;
        if (_watching)
        {
            return;
        }

        _watching = true;
        Clipboard.ContentChanged += (_, _) =>
        {
            if (Environment.TickCount64 < _suppressUntil)
            {
                return;
            }

            _ = Capture();
        };
        _ = Capture();
    }

    public static async Task ApplyAsync(string text, byte[]? png)
    {
        _suppressUntil = Environment.TickCount64 + 800;
        try
        {
            RandomAccessStreamReference? bitmap = null;
            if (png is { Length: > 0 })
            {
                var stream = new InMemoryRandomAccessStream();
                // Completing this write on the UI thread can deadlock SetContent.
                await stream.WriteAsync(png.AsBuffer()).AsTask().ConfigureAwait(false);
                stream.Seek(0);
                bitmap = RandomAccessStreamReference.CreateFromStream(stream);
            }

            void Commit()
            {
                _suppressUntil = Environment.TickCount64 + 800;
                if (text.Length == 0 && bitmap == null)
                {
                    return;
                }

                var package = new DataPackage();
                if (text.Length > 0)
                {
                    package.SetText(text);
                }

                if (bitmap != null)
                {
                    package.SetBitmap(bitmap);
                }

                Clipboard.SetContent(package);
                try
                {
                    Clipboard.Flush();
                }
                catch
                {
                    // Another app can hold the clipboard. The content is still set for this process.
                }
            }

            if (_queue is { } queue && !queue.HasThreadAccess)
            {
                queue.TryEnqueue(Commit);
            }
            else
            {
                Commit();
            }
        }
        catch
        {
            // The clipboard can be busy while another app writes it.
        }
    }

    static async Task Capture()
    {
        var generation = Interlocked.Increment(ref _generation);
        try
        {
            await YieldUi();
            var view = Clipboard.GetContent();
            var hadText = view.Contains(StandardDataFormats.Text);
            var hadImage = view.Contains(StandardDataFormats.Bitmap);
            string text = "";
            if (hadText)
            {
                text = await view.GetTextAsync();
                await YieldUi();
            }

            byte[]? png = null;
            var imageTooBig = false;
            if (hadImage)
            {
                (png, imageTooBig) = await EncodePng(view);
            }

            if (generation != Volatile.Read(ref _generation))
            {
                return;
            }

            var textTooBig = System.Text.Encoding.UTF8.GetByteCount(text) > MaxTextBytes;
            if (textTooBig)
            {
                text = "";
            }

            // A copy that is too large should not wipe the previous snapshot.
            if (text.Length == 0 && png == null && (textTooBig || imageTooBig))
            {
                return;
            }

            var snapshotText = text;
            var snapshotPng = png;
            _queue?.TryEnqueue(() => _publish?.Invoke(snapshotText, snapshotPng));
        }
        catch
        {
            // The clipboard can be busy while another app writes it.
        }
    }

    static async Task<(byte[]? Png, bool TooBig)> EncodePng(DataPackageView view)
    {
        await YieldUi();
        var reference = await view.GetBitmapAsync();
        using var stream = await reference.OpenReadAsync();
        var decoder = await BitmapDecoder.CreateAsync(stream);
        var bitmap = await decoder.GetSoftwareBitmapAsync(BitmapPixelFormat.Bgra8, BitmapAlphaMode.Premultiplied);
        using var output = new InMemoryRandomAccessStream();
        var encoder = await BitmapEncoder.CreateAsync(BitmapEncoder.PngEncoderId, output);
        encoder.SetSoftwareBitmap(bitmap);
        await encoder.FlushAsync();
        var size = output.Size;
        if (size > MaxImageBytes)
        {
            return (null, true);
        }

        if (size == 0)
        {
            return (null, false);
        }

        output.Seek(0);
        var bytes = new byte[size];
        using var reader = new DataReader(output);
        await reader.LoadAsync((uint)size);
        reader.ReadBytes(bytes);
        return (bytes, false);
    }

    static Task YieldUi()
    {
        var queue = _queue;
        if (queue == null || queue.HasThreadAccess)
        {
            return Task.CompletedTask;
        }

        var done = new TaskCompletionSource();
        if (!queue.TryEnqueue(() => done.TrySetResult()))
        {
            done.TrySetResult();
        }

        return done.Task;
    }
}
