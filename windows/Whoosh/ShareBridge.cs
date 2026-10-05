using System.IO.Pipes;
using System.Text;

namespace Whoosh;

/// Sends shared paths to the Whoosh window that is already open.
internal static class ShareBridge
{
    const string PipeName = "Whoosh.Share";

    public static void Listen(Action<string[]> deliver)
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
                    while (await reader.ReadLineAsync() is string line)
                    {
                        if (line.Length == 0)
                        {
                            break;
                        }

                        paths.Add(line);
                    }

                    if (paths.Count > 0)
                    {
                        deliver(paths.ToArray());
                    }
                }
                catch
                {
                    await Task.Delay(200);
                }
            }
        });
    }

    public static bool Send(IReadOnlyList<string> paths)
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
                foreach (var path in paths)
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
