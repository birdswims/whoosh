namespace WhooshSetup;

static class Program
{
    [STAThread]
    static void Main(string[] args)
    {
        ApplicationConfiguration.Initialize();
        Application.SetDefaultFont(new Font("Segoe UI", 9f));
        var quiet = args.Any(arg => arg is "/quiet" or "/S" or "/silent" or "--quiet");
        if (quiet)
        {
            try
            {
                var directory = Installer.Install(null);
                Installer.Launch(directory);
            }
            catch (Exception error)
            {
                try
                {
                    File.WriteAllText(Path.Combine(Path.GetTempPath(), "WhooshSetup.log"), error.ToString());
                }
                catch
                {
                    // The exit code still reports the failure.
                }

                Environment.ExitCode = 1;
            }

            return;
        }

        Application.Run(new SetupForm());
    }
}
