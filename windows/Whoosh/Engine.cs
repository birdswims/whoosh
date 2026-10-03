using System.Diagnostics;
using System.Text;
using System.Text.Json;

namespace Whoosh;

/// <summary>
/// Talks to the bundled <c>whoosh-core.exe app</c> process. One JSON command
/// per line in, one JSON event per line out.
/// </summary>
public sealed class Engine
{
    static readonly JsonSerializerOptions Json = new()
    {
        DefaultIgnoreCondition = System.Text.Json.Serialization.JsonIgnoreCondition.WhenWritingNull,
        PropertyNameCaseInsensitive = true,
    };

    public event Action<WireEvent>? EventReceived;
    public event Action<string>? Exited;

    Process? _process;
    StreamWriter? _input;
    readonly object _writeLock = new();
    readonly object _errorLock = new();
    readonly StringBuilder _stderr = new();
    bool _intentional;

    public void Start()
    {
        if (_process is { HasExited: false })
        {
            return;
        }

        var binary = Locate();
        if (binary == null)
        {
            Exited?.Invoke("The Whoosh engine is missing from the app.");
            return;
        }

        var process = new Process();
        process.StartInfo = new ProcessStartInfo
        {
            FileName = binary,
            UseShellExecute = false,
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            CreateNoWindow = true,
            StandardInputEncoding = new UTF8Encoding(false),
            StandardOutputEncoding = new UTF8Encoding(false),
            StandardErrorEncoding = new UTF8Encoding(false),
        };
        process.StartInfo.ArgumentList.Add("app");
        process.EnableRaisingEvents = true;
        process.Exited += (_, _) =>
        {
            if (_intentional || !ReferenceEquals(_process, process))
            {
                return;
            }

            Exited?.Invoke(Tail());
        };

        try
        {
            process.Start();
        }
        catch (Exception error)
        {
            Exited?.Invoke(error.Message);
            return;
        }

        _intentional = false;
        _process = process;
        _input = process.StandardInput;
        var output = process.StandardOutput;
        var errorStream = process.StandardError;
        var stdout = new Thread(() => ReadLines(output)) { IsBackground = true, Name = "whoosh-stdout" };
        var stderr = new Thread(() => ReadErrors(errorStream)) { IsBackground = true, Name = "whoosh-stderr" };
        stdout.Start();
        stderr.Start();
    }

    public void Send(Command command)
    {
        var input = _input;
        if (input == null)
        {
            return;
        }

        string line;
        try
        {
            line = JsonSerializer.Serialize(command, Json);
        }
        catch
        {
            return;
        }

        lock (_writeLock)
        {
            try
            {
                input.WriteLine(line);
                input.Flush();
            }
            catch
            {
                // The exit handler reports a dead engine.
            }
        }
    }

    public void StopAndWait()
    {
        _intentional = true;
        try
        {
            Send(new Command { Id = "bye", Op = "shutdown" });
            _input?.Close();
        }
        catch
        {
            // The process is going away either way.
        }

        _input = null;
        var process = _process;
        if (process == null)
        {
            return;
        }

        try
        {
            if (!process.HasExited && !process.WaitForExit(800))
            {
                process.Kill(entireProcessTree: true);
                process.WaitForExit(500);
            }
        }
        catch
        {
            // Already gone.
        }
    }

    string Tail()
    {
        lock (_errorLock)
        {
            return _stderr.ToString();
        }
    }

    void ReadLines(StreamReader reader)
    {
        try
        {
            while (reader.ReadLine() is string line)
            {
                if (line.Length == 0)
                {
                    continue;
                }

                WireEvent? ev;
                try
                {
                    ev = JsonSerializer.Deserialize<WireEvent>(line, Json);
                }
                catch
                {
                    continue;
                }

                if (ev != null)
                {
                    EventReceived?.Invoke(ev);
                }
            }
        }
        catch
        {
            // The process closed the pipe.
        }
    }

    void ReadErrors(StreamReader reader)
    {
        try
        {
            var buffer = new char[2048];
            while (true)
            {
                var count = reader.Read(buffer, 0, buffer.Length);
                if (count <= 0)
                {
                    break;
                }

                lock (_errorLock)
                {
                    _stderr.Append(buffer, 0, count);
                    if (_stderr.Length > 12_000)
                    {
                        _stderr.Remove(0, _stderr.Length - 8_000);
                    }
                }
            }
        }
        catch
        {
            // The process closed the pipe.
        }
    }

    public static string? Locate()
    {
        var candidates = new List<string>();
        var env = Environment.GetEnvironmentVariable("WHOOSH_BIN");
        if (!string.IsNullOrWhiteSpace(env))
        {
            candidates.Add(env);
        }

        candidates.Add(Path.Combine(AppContext.BaseDirectory, "whoosh-core.exe"));
        var dir = new DirectoryInfo(AppContext.BaseDirectory);
        for (var depth = 0; depth < 8 && dir != null; depth++, dir = dir.Parent)
        {
            candidates.Add(Path.Combine(dir.FullName, "target", "release", "whoosh.exe"));
            candidates.Add(Path.Combine(dir.FullName, "target", "debug", "whoosh.exe"));
        }

        foreach (var path in candidates)
        {
            if (File.Exists(path))
            {
                return path;
            }
        }

        return null;
    }
}
