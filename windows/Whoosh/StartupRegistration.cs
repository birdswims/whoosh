using Microsoft.Win32;

namespace Whoosh;

/// HKCU Run entry. The value includes --background so a sign-in launch can stay
/// hidden when "Run in the background" is on. A Start-menu launch has no flag
/// and always shows the window.
internal static class StartupRegistration
{
    const string KeyPath = @"Software\Microsoft\Windows\CurrentVersion\Run";
    const string ValueName = "Whoosh";

    public static bool Set(bool enabled)
    {
        try
        {
            using var key = Registry.CurrentUser.CreateSubKey(KeyPath);
            if (key == null)
            {
                return false;
            }

            if (!enabled)
            {
                key.DeleteValue(ValueName, throwOnMissingValue: false);
                return true;
            }

            var exe = Environment.ProcessPath;
            if (string.IsNullOrEmpty(exe))
            {
                return false;
            }

            key.SetValue(ValueName, "\"" + exe.Replace("\"", "") + "\" --background");
            return true;
        }
        catch
        {
            return false;
        }
    }

    public static void Remove()
    {
        try
        {
            using var key = Registry.CurrentUser.OpenSubKey(KeyPath, writable: true);
            key?.DeleteValue(ValueName, throwOnMissingValue: false);
        }
        catch
        {
            // The Run value can be removed by hand if this fails.
        }
    }
}
