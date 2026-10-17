namespace Prostor.Core;

/// <summary>Splits command lines like those found in UninstallString or Run keys.</summary>
public static class CommandLine
{
    public static (string file, string args) Split(string command)
    {
        if (string.IsNullOrWhiteSpace(command)) return ("", "");
        string cmd = Environment.ExpandEnvironmentVariables(command.Trim());

        if (cmd.StartsWith('"'))
        {
            int end = cmd.IndexOf('"', 1);
            if (end > 0) return (cmd.Substring(1, end - 1), cmd.Substring(end + 1).Trim());
            return (cmd.Trim('"'), "");
        }

        // Unquoted path that may contain spaces: take the shortest prefix that resolves to an existing file.
        int idx = 0;
        while (true)
        {
            int sp = cmd.IndexOf(' ', idx);
            string candidate = sp < 0 ? cmd : cmd.Substring(0, sp);
            string resolved = Resolve(candidate);
            if (resolved != null) return (resolved, sp < 0 ? "" : cmd.Substring(sp + 1).Trim());
            if (sp < 0) break;
            idx = sp + 1;
        }

        int first = cmd.IndexOf(' ');
        return first < 0 ? (cmd, "") : (cmd.Substring(0, first), cmd.Substring(first + 1).Trim());
    }

    /// <summary>Returns the executable referenced by a command line, if it exists on disk.</summary>
    public static string ExtractExecutable(string command)
    {
        var (file, _) = Split(command);
        if (string.IsNullOrEmpty(file)) return null;
        return Resolve(file);
    }

    private static string Resolve(string candidate)
    {
        try
        {
            if (candidate.Length == 0 || candidate.IndexOfAny(Path.GetInvalidPathChars()) >= 0) return null;
            if (File.Exists(candidate)) return candidate;
            if (!candidate.EndsWith(".exe", StringComparison.OrdinalIgnoreCase) && File.Exists(candidate + ".exe"))
                return candidate + ".exe";
            if (!candidate.Contains('\\'))
            {
                foreach (var dir in new[] { Environment.SystemDirectory, Environment.GetFolderPath(Environment.SpecialFolder.Windows) })
                {
                    string p = Path.Combine(dir, candidate);
                    if (File.Exists(p)) return p;
                    if (File.Exists(p + ".exe")) return p + ".exe";
                }
            }
        }
        catch { }
        return null;
    }
}
