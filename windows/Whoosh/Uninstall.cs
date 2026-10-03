using System.Diagnostics;
using System.Runtime.InteropServices;
using Microsoft.Win32;

namespace Whoosh;

internal static class Uninstall
{
    public static void Run()
    {
        var dir = Path.GetFullPath(AppContext.BaseDirectory).TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
        var expected = Path.GetFullPath(Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "Programs",
            "Whoosh"));
        if (!string.Equals(dir, expected, StringComparison.OrdinalIgnoreCase))
        {
            MessageBox(IntPtr.Zero, "This copy of Whoosh was not installed for the current user, so it was left in place.", "Whoosh", 0x40);
            return;
        }

        try
        {
            var link = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.Programs), "Whoosh.lnk");
            if (File.Exists(link))
            {
                File.Delete(link);
            }
        }
        catch
        {
            // The folder removal still uninstalls the app.
        }

        try
        {
            Registry.CurrentUser.DeleteSubKeyTree(@"Software\Microsoft\Windows\CurrentVersion\Uninstall\Whoosh", throwOnMissingSubKey: false);
        }
        catch
        {
            // The Apps list entry can be removed by hand if this fails.
        }

        try
        {
            Process.Start(new ProcessStartInfo
            {
                FileName = "netsh",
                Arguments = "advfirewall firewall delete rule name=\"Whoosh\"",
                CreateNoWindow = true,
                UseShellExecute = false,
            });
        }
        catch
        {
            // The firewall rule is optional.
        }

        foreach (var name in new[] { "whoosh-core" })
        {
            foreach (var process in Process.GetProcessesByName(name))
            {
                try
                {
                    process.Kill(entireProcessTree: true);
                }
                catch
                {
                    // Already exiting.
                }
            }
        }

        foreach (var process in Process.GetProcessesByName("Whoosh"))
        {
            if (process.Id == Environment.ProcessId)
            {
                continue;
            }

            try
            {
                process.Kill(entireProcessTree: true);
            }
            catch
            {
                // Already exiting.
            }
        }

        // This process has the install folder locked. A helper removes it after we exit.
        Process.Start(new ProcessStartInfo
        {
            FileName = "cmd.exe",
            Arguments = "/c ping -n 3 127.0.0.1 >nul & rmdir /s /q \"" + dir + "\"",
            CreateNoWindow = true,
            UseShellExecute = false,
            WindowStyle = ProcessWindowStyle.Hidden,
        });
    }

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    static extern int MessageBox(IntPtr owner, string text, string caption, uint type);
}
