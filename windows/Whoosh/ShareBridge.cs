using System.IO.Pipes;
using System.Text;

namespace Whoosh;

/// Sends shared paths, or a show request, to the Whoosh window that is already open.
internal static class ShareBridge
{
    const string PipeName = "Whoosh.Share";
    const string ShowCommand = ":show";

    public static void Listen(Action<string[], bool> deliver)
    {
        _ = Task.Run(async () =>
        {
            while (true)
            {
                try
                {
                    await using var server = new NamedPipeServerStream(
                        PipeName,
                        PipeDirection.In,
                        1,
                        PipeTransmissionMode.Byte,
                        PipeOptions.Asynchronous);
                    await server.WaitForConnectionAsync();
                    using var reader = new StreamReader(server, Encoding.UTF8);
                    var paths = new List<string>();
                    var show = false;
                    while (await reader.ReadLineAsync() is string line)
                    {
                        if (line.Length == 0)
                        {
                            break;
                        }

                        if (line.TrimEnd('\r') == ShowCommand)
                        {
                            show = true;
                            continue;
                        }

                        paths.Add(line);
                    }

                    if (paths.Count > 0)
                    {
                        show = true;
                    }

                    if (paths.Count > 0 || show)
                    {
                        deliver(paths.ToArray(), show);
                    }
                }
                catch
                {
                    await Task.Delay(200);
                }
            }
        });
    }

    public static bool Send(IReadOnlyList<string> paths) => Write(paths);

    public static bool Show() => Write([ShowCommand]);

    static bool Write(IReadOnlyList<string> lines)
    {
        for (var attempt = 0; attempt < 40; attempt++)
        {
            try
            {
                using var client = new NamedPipeClientStream(".", PipeName, PipeDirection.Out);
                client.Connect(250);
                using var writer = new StreamWriter(client, new UTF8Encoding(encoderShouldEmitUTF8Identifier: false))
                {
                    AutoFlush = true,
                };
                foreach (var path in lines)
                {
                    var line = path.Replace("\r", "").Replace("\n", "");
                    if (line.Length > 0)
                    {
                        writer.WriteLine(line);
                    }
                }

                writer.WriteLine();
                return true;
            }
            catch
            {
                Thread.Sleep(100);
            }
        }

        return false;
    }
}
