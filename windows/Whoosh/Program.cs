using System.Diagnostics;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;

namespace Whoosh;

public static class Program
{
    [STAThread]
    public static void Main(string[] args)
    {
        if (args.Any(arg => arg is "--uninstall" or "/uninstall"))
        {
            Uninstall.Run();
            return;
        }

        if (args.Any(arg => arg is "--register-share"))
        {
            Environment.Exit(ShareIdentity.Register() ? 0 : 1);
        }

        WinRT.ComWrappersSupport.InitializeComWrappers();
        var restarted = args.Any(arg => arg is "--identity-restart");
        ShareHandoff? share;
        try
        {
            share = ShareActivation.Take();
        }
        catch
        {
            return;
        }

        if (share is null && !restarted && !ShareIdentity.HasIdentity() && ShareIdentity.Register())
        {
            Relaunch(args);
            return;
        }

        var incoming = share is not null ? share.Paths : FileArgs(args);
        SingleInstance.SetAppId();
        if (!SingleInstance.Acquire())
        {
            if (incoming.Count > 0 && !ShareBridge.Send(incoming))
            {
                share?.Fail("Whoosh is not ready to receive files.");
                return;
            }

            share?.Complete();
            return;
        }

        App.LaunchFiles = incoming.ToArray();
        App.PendingShare = share;
        Application.Start(_ =>
        {
            var context = new DispatcherQueueSynchronizationContext(DispatcherQueue.GetForCurrentThread());
            SynchronizationContext.SetSynchronizationContext(context);
            new App();
        });
    }

    static IReadOnlyList<string> FileArgs(string[] args)
    {
        return args
            .Where(arg => !arg.StartsWith('-') && !arg.StartsWith('/') && (File.Exists(arg) || Directory.Exists(arg)))
            .ToArray();
    }

    static void Relaunch(string[] args)
    {
        var executable = Environment.ProcessPath;
        if (string.IsNullOrEmpty(executable))
        {
            return;
        }

        var forwarded = args.Where(arg => arg != "--identity-restart").Append("--identity-restart");
        Process.Start(new ProcessStartInfo
        {
            FileName = executable,
            Arguments = string.Join(" ", forwarded.Select(Quote)),
            WorkingDirectory = AppContext.BaseDirectory,
            UseShellExecute = true,
        });
    }

    static string Quote(string value) => "\"" + value.Replace("\"", "") + "\"";
}
