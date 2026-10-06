using Microsoft.UI.Xaml;

namespace Whoosh;

public partial class App : Application
{
    public static AppModel Model { get; private set; } = null!;
    public static MainWindow? Main { get; set; }
    public static string[] LaunchFiles { get; set; } = [];
    public static ShareHandoff? PendingShare { get; set; }
    public static bool LaunchBackground { get; set; }

    public App()
    {
        InitializeComponent();
        UnhandledException += (_, e) =>
        {
            e.Handled = true;
        };
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        Model = new AppModel();
        var window = new MainWindow();
        Main = window;
        window.ApplyLaunchVisibility();
        if (!window.StartedHidden)
        {
            window.Activate();
        }

        if (Model.RunInBackground)
        {
            TrayIcon.Add();
        }

        StartupRegistration.Set(Model.LaunchAtLogin);
        ShareBridge.Listen((paths, show) =>
        {
            window.DispatcherQueue.TryEnqueue(() =>
            {
                if (paths.Length > 0)
                {
                    Model.AddPaths(paths);
                }

                if (show)
                {
                    window.Reveal();
                }
            });
        });
        Model.Start();
        if (LaunchFiles.Length > 0)
        {
            Model.AddPaths(LaunchFiles);
        }

        PendingShare?.Complete();
        PendingShare = null;
    }
}
