using System.Diagnostics;
using System.IO.Pipes;
using System.Text;

namespace Prostor.Core;

/// <summary>
/// Keeps one window per user session: a second launch (e.g. "Анализировать в «Простор»" from Explorer)
/// hands its arguments to the running instance and exits.
/// </summary>
public static class SingleInstance
{
    private static Mutex _mutex;
    private static readonly string Id = $"Prostor.{Environment.UserName}.{Process.GetCurrentProcess().SessionId}";

    /// <summary>True when this is the first instance.</summary>
    /// <param name="waitMs">How long to wait for a previous instance to exit (used after a restart).</param>
    public static bool TryAcquire(int waitMs = 0)
    {
        try
        {
            _mutex = new Mutex(true, @"Local\" + Id, out bool created);
            if (created || waitMs <= 0) return created;
            try
            {
                return _mutex.WaitOne(waitMs);
            }
            catch (AbandonedMutexException)
            {
                return true; // the previous instance exited and the mutex is ours now
            }
        }
        catch
        {
            return false;
        }
    }

    public static bool SendToRunning(string[] args)
    {
        try
        {
            using var client = new NamedPipeClientStream(".", Id, PipeDirection.Out);
            client.Connect(1500);
            var data = Encoding.UTF8.GetBytes(string.Join("\n", args));
            client.Write(data, 0, data.Length);
            client.Flush();
            return true;
        }
        catch
        {
            return false;
        }
    }

    /// <summary>Receives arguments from later launches; <paramref name="onArgs"/> is called on a background thread.</summary>
    public static void Listen(Action<string[]> onArgs)
    {
        var thread = new Thread(() =>
        {
            while (true)
            {
                try
                {
                    using var server = new NamedPipeServerStream(Id, PipeDirection.In, 1, PipeTransmissionMode.Byte);
                    server.WaitForConnection();
                    using var ms = new MemoryStream();
                    server.CopyTo(ms);
                    string text = Encoding.UTF8.GetString(ms.ToArray());
                    onArgs(text.Length == 0 ? Array.Empty<string>() : text.Split('\n'));
                }
                catch
                {
                    Thread.Sleep(500);
                }
            }
        })
        { IsBackground = true, Name = "Prostor.SingleInstance" };
        thread.Start();
    }
}
