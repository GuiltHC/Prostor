using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;

namespace Prostor.Core;

public sealed record CmdResult(int ExitCode, string Output)
{
    public bool Ok => ExitCode == 0;
}

/// <summary>Runs console tools (reg, powercfg, compact, vssadmin, pnputil, dism, winget) hidden, capturing their output.</summary>
public static class Cmd
{
    static Cmd() => Encoding.RegisterProvider(CodePagesEncodingProvider.Instance);

    [DllImport("kernel32.dll")]
    private static extern uint GetOEMCP();

    /// <summary>Encoding of classic console tools (CP866 on Russian Windows).</summary>
    public static Encoding Oem
    {
        get
        {
            try { return Encoding.GetEncoding((int)GetOEMCP()); }
            catch { return Encoding.UTF8; }
        }
    }

    public static string SystemTool(string exe) => Path.Combine(Environment.SystemDirectory, exe);

    public static CmdResult Run(string file, string args, Encoding encoding = null, int timeoutMs = 120_000,
        Action<string> onLine = null, CancellationToken ct = default)
    {
        var psi = new ProcessStartInfo(file, args)
        {
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            StandardOutputEncoding = encoding ?? Oem,
            StandardErrorEncoding = encoding ?? Oem,
        };
        using var p = new Process { StartInfo = psi };
        var sb = new StringBuilder();
        var gate = new object();
        void OnData(object _, DataReceivedEventArgs e)
        {
            if (e.Data == null) return;
            lock (gate) sb.AppendLine(e.Data);
            onLine?.Invoke(e.Data);
        }
        p.OutputDataReceived += OnData;
        p.ErrorDataReceived += OnData;
        p.Start();
        p.BeginOutputReadLine();
        p.BeginErrorReadLine();

        var sw = Stopwatch.StartNew();
        while (!p.WaitForExit(200))
        {
            if (ct.IsCancellationRequested || sw.ElapsedMilliseconds > timeoutMs)
            {
                try { p.Kill(true); } catch { }
                ct.ThrowIfCancellationRequested();
                throw new TimeoutException($"{Path.GetFileName(file)} не ответил вовремя");
            }
        }
        p.WaitForExit();
        lock (gate) return new CmdResult(p.ExitCode, sb.ToString());
    }

    public static Task<CmdResult> RunAsync(string file, string args, Encoding encoding = null, int timeoutMs = 120_000,
        Action<string> onLine = null, CancellationToken ct = default) =>
        Task.Run(() => Run(file, args, encoding, timeoutMs, onLine, ct), ct);

    public static string Quote(string s) => "\"" + s.Replace("\"", "\\\"") + "\"";
}
