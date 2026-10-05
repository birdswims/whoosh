using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Security.Cryptography.X509Certificates;
using System.Text;

namespace Whoosh;

/// Registers the sparse package that puts Whoosh in the Windows Share window.
internal static class ShareIdentity
{
    public const string PackageName = "Whoosh.Desktop";

    public static bool HasIdentity()
    {
        var length = 0;
        var result = GetCurrentPackageFullName(ref length, null);
        return result != 15700;
    }

    public static bool Register()
    {
        var directory = InstallDirectory();
        var package = Path.Combine(directory, "Whoosh.identity.msix");
        var certificate = Path.Combine(directory, "Whoosh.cer");
        if (!File.Exists(package) || !File.Exists(certificate))
        {
            return false;
        }

        try
        {
            using var parsed = new X509Certificate2(certificate);
            Trust(parsed);
        }
        catch
        {
            // A certificate that is already trusted still lets the package install.
        }

        var command = "$ErrorActionPreference = 'Stop'\n"
            + "$found = @(Get-AppxPackage -Name '" + PackageName + "')\n"
            + "foreach ($pkg in $found) { Remove-AppxPackage -Package $pkg.PackageFullName }\n"
            + "Add-AppxPackage -Path " + Quote(package) + " -ExternalLocation " + Quote(directory) + "\n";
        return Run(command, 60_000);
    }

    static void Trust(X509Certificate2 certificate)
    {
        foreach (var name in new[] { StoreName.Root, StoreName.TrustedPeople })
        {
            try
            {
                using var store = new X509Store(name, StoreLocation.CurrentUser);
                store.Open(OpenFlags.ReadWrite);
                store.Add(certificate);
            }
            catch
            {
                // CurrentUser\Root can refuse the certificate. TrustedPeople still counts.
            }
        }
    }

    public static void Unregister()
    {
        Run("$ErrorActionPreference = 'Stop'\n"
            + "$found = @(Get-AppxPackage -Name '" + PackageName + "')\n"
            + "foreach ($pkg in $found) { Remove-AppxPackage -Package $pkg.PackageFullName }\n", 60_000);
    }

    static string InstallDirectory()
    {
        return AppContext.BaseDirectory.TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
    }

    static string Quote(string value) => "'" + value.Replace("'", "''") + "'";

    static bool Run(string command, int timeoutMs)
    {
        try
        {
            var encoded = Convert.ToBase64String(Encoding.Unicode.GetBytes(command));
            using var process = Process.Start(new ProcessStartInfo
            {
                FileName = "powershell.exe",
                Arguments = "-NoProfile -NonInteractive -EncodedCommand " + encoded,
                UseShellExecute = false,
                CreateNoWindow = true,
            });
            if (process is null || !process.WaitForExit(timeoutMs))
            {
                try
                {
                    process?.Kill(entireProcessTree: true);
                }
                catch
                {
                    // The registration process already exited.
                }

                return false;
            }

            return process.ExitCode == 0;
        }
        catch
        {
            return false;
        }
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern int GetCurrentPackageFullName(ref int length, StringBuilder? packageFullName);
}
