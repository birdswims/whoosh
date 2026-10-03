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

        SingleInstance.SetAppId();
        if (!SingleInstance.Acquire())
        {
            return;
        }

        App.LaunchFiles = args
            .Where(arg => !arg.StartsWith('-') && !arg.StartsWith('/') && (File.Exists(arg) || Directory.Exists(arg)))
            .ToArray();

        WinRT.ComWrappersSupport.InitializeComWrappers();
        Application.Start(_ =>
        {
            var context = new DispatcherQueueSynchronizationContext(DispatcherQueue.GetForCurrentThread());
            SynchronizationContext.SetSynchronizationContext(context);
            new App();
        });
    }
}
