using System.Diagnostics;
using System.IO.Compression;
using System.Reflection;
using Microsoft.Win32;

namespace WhooshSetup;

static class Installer
{
    public static string InstallDirectory => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "Programs",
        "Whoosh");

    public static string Install(IProgress<string>? progress)
    {
        progress?.Report("Preparing…");
        using var resource = Assembly.GetExecutingAssembly().GetManifestResourceStream("whoosh-payload.zip")
            ?? throw new InvalidOperationException("The installer is missing its files.");

        var staging = Path.Combine(Path.GetTempPath(), "whoosh-setup-" + Guid.NewGuid().ToString("n"));
        Directory.CreateDirectory(staging);
        try
        {
            var zipPath = Path.Combine(staging, "payload.zip");
            using (var output = File.Create(zipPath))
            {
                resource.CopyTo(output);
            }

            progress?.Report("Unpacking…");
            var extracted = Path.Combine(staging, "files");
            ZipFile.ExtractToDirectory(zipPath, extracted);
            if (!File.Exists(Path.Combine(extracted, "Whoosh.exe")) || !File.Exists(Path.Combine(extracted, "whoosh-core.exe")))
            {
                throw new InvalidOperationException("The installer payload is incomplete.");
            }

            progress?.Report("Closing Whoosh…");
            StopRunning();

            var destination = InstallDirectory;
            Directory.CreateDirectory(destination);
            progress?.Report("Copying files…");
            CopyAll(extracted, destination);

            progress?.Report("Adding the Start menu shortcut…");
            CreateShortcut(destination);
            RegisterUninstall(destination);
            TryFirewall(destination);
            progress?.Report("Installed.");
            return destination;
        }
        finally
        {
            try
            {
                Directory.Delete(staging, recursive: true);
            }
            catch
            {
                // The temp folder is disposable.
            }
        }
    }

    public static void Launch(string directory)
    {
        Process.Start(new ProcessStartInfo
        {
            FileName = Path.Combine(directory, "Whoosh.exe"),
            WorkingDirectory = directory,
            UseShellExecute = true,
        });
    }

    static void StopRunning()
    {
        foreach (var name in new[] { "Whoosh", "whoosh-core" })
        {
            foreach (var process in Process.GetProcessesByName(name))
            {
                try
                {
                    if (process.Id == Environment.ProcessId)
                    {
                        continue;
                    }

                    process.Kill(entireProcessTree: true);
                    process.WaitForExit(4000);
                }
                catch
                {
                    // A file lock surfaces again on copy, which retries.
                }
            }
        }
    }

    static void CopyAll(string source, string destination)
    {
        Directory.CreateDirectory(destination);
        foreach (var file in Directory.GetFiles(source))
        {
            CopyRetry(file, Path.Combine(destination, Path.GetFileName(file)));
        }

        foreach (var directory in Directory.GetDirectories(source))
        {
            CopyAll(directory, Path.Combine(destination, Path.GetFileName(directory)));
        }
    }

    static void CopyRetry(string source, string destination)
    {
        for (var attempt = 0; ; attempt++)
        {
            try
            {
                File.Copy(source, destination, overwrite: true);
                return;
            }
            catch (IOException) when (attempt < 8)
            {
                Thread.Sleep(250);
            }
        }
    }

    static void CreateShortcut(string directory)
    {
        var programs = Environment.GetFolderPath(Environment.SpecialFolder.Programs);
        Directory.CreateDirectory(programs);
        var link = Path.Combine(programs, "Whoosh.lnk");
        var shellType = Type.GetTypeFromProgID("WScript.Shell")
            ?? throw new InvalidOperationException("Windows Script Host is unavailable.");
        dynamic shell = Activator.CreateInstance(shellType)!;
        dynamic shortcut = shell.CreateShortcut(link);
        shortcut.TargetPath = Path.Combine(directory, "Whoosh.exe");
        shortcut.WorkingDirectory = directory;
        shortcut.Description = "Send files, photos, and videos";
        shortcut.Save();
    }

    static void RegisterUninstall(string directory)
    {
        var exe = Path.Combine(directory, "Whoosh.exe");
        using var key = Registry.CurrentUser.CreateSubKey(@"Software\Microsoft\Windows\CurrentVersion\Uninstall\Whoosh");
        key.SetValue("DisplayName", "Whoosh");
        key.SetValue("DisplayVersion", "0.1.0");
        key.SetValue("Publisher", "Whoosh");
        key.SetValue("InstallLocation", directory);
        key.SetValue("DisplayIcon", exe);
        key.SetValue("UninstallString", $"\"{exe}\" --uninstall");
        key.SetValue("QuietUninstallString", $"\"{exe}\" --uninstall");
        key.SetValue("NoModify", 1, RegistryValueKind.DWord);
        key.SetValue("NoRepair", 1, RegistryValueKind.DWord);
        long bytes = 0;
        foreach (var file in Directory.EnumerateFiles(directory, "*", SearchOption.AllDirectories))
        {
            try
            {
                bytes += new FileInfo(file).Length;
            }
            catch
            {
                // Skip a file that disappears mid-count.
            }
        }

        key.SetValue("EstimatedSize", (int)Math.Min(int.MaxValue, bytes / 1024), RegistryValueKind.DWord);
    }

    static void TryFirewall(string directory)
    {
        var engine = Path.Combine(directory, "whoosh-core.exe");
        try
        {
            Process.Start(new ProcessStartInfo
            {
                FileName = "netsh",
                Arguments = $"advfirewall firewall delete rule name=\"Whoosh\" program=\"{engine}\"",
                CreateNoWindow = true,
                UseShellExecute = false,
            })?.WaitForExit(4000);
            Process.Start(new ProcessStartInfo
            {
                FileName = "netsh",
                Arguments = $"advfirewall firewall add rule name=\"Whoosh\" dir=in action=allow program=\"{engine}\" enable=yes profile=private,domain",
                CreateNoWindow = true,
                UseShellExecute = false,
            })?.WaitForExit(4000);
        }
        catch
        {
            // Without an administrator, Windows asks on the first listen instead.
        }
    }
}
