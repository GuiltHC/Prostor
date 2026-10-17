using System.Text.Json;
using System.Text.Json.Serialization;
using Prostor.Core;

namespace Prostor.Services;

public enum UndoKind
{
    None,
    /// <summary>UndoData: .reg backup files to import.</summary>
    RegImport,
    /// <summary>UndoData: original paths of items moved to the Recycle Bin.</summary>
    RecycleRestore,
    /// <summary>UndoData: pairs "task name|xml backup path".</summary>
    TaskRestore,
    /// <summary>UndoData: folders compressed with NTFS (WOF) compression.</summary>
    Decompress,
    /// <summary>UndoData: [original path, new location].</summary>
    MoveBack,
    /// <summary>Re-enables hibernation.</summary>
    Hibernation,
    /// <summary>UndoData: [startup item key, delayed task name].</summary>
    DelayedStart,
}

public sealed class JournalEntry
{
    public string Id { get; set; } = Guid.NewGuid().ToString("N");
    public DateTime Time { get; set; } = DateTime.Now;
    /// <summary>uninstall, entry, leftovers, force, cleanup, autoclean, delete, duplicates, compress, move, startup, system.</summary>
    public string Kind { get; set; }
    public string Title { get; set; }
    public string Detail { get; set; }
    public long Bytes { get; set; }
    [JsonConverter(typeof(JsonStringEnumConverter))]
    public UndoKind Undo { get; set; }
    public List<string> UndoData { get; set; } = new();
    public bool Undone { get; set; }
}

/// <summary>History of everything the app changed, with what is needed to roll it back.</summary>
public static class ActionJournal
{
    private const int MaxEntries = 500;
    private static readonly object Lock = new();
    private static readonly JsonSerializerOptions Json = new() { WriteIndented = true };

    public static event Action Changed;

    public static string FilePath => Path.Combine(AppSettings.DataFolder, "journal.json");

    public static List<JournalEntry> Load()
    {
        lock (Lock)
        {
            try
            {
                if (File.Exists(FilePath))
                    return JsonSerializer.Deserialize<List<JournalEntry>>(File.ReadAllText(FilePath)) ?? new();
            }
            catch { }
            return new();
        }
    }

    public static void Add(JournalEntry entry)
    {
        lock (Lock)
        {
            var list = Load();
            list.Insert(0, entry);
            if (list.Count > MaxEntries) list.RemoveRange(MaxEntries, list.Count - MaxEntries);
            Save(list);
        }
        Changed?.Invoke();
    }

    public static void Add(string kind, string title, string detail = null, long bytes = 0, UndoKind undo = UndoKind.None,
        IEnumerable<string> undoData = null) =>
        Add(new JournalEntry
        {
            Kind = kind, Title = title, Detail = detail, Bytes = bytes, Undo = undo,
            UndoData = undoData?.ToList() ?? new()
        });

    public static void MarkUndone(string id)
    {
        lock (Lock)
        {
            var list = Load();
            var e = list.FirstOrDefault(x => x.Id == id);
            if (e == null) return;
            e.Undone = true;
            Save(list);
        }
        Changed?.Invoke();
    }

    public static void Clear()
    {
        lock (Lock) Save(new List<JournalEntry>());
        Changed?.Invoke();
    }

    private static void Save(List<JournalEntry> list)
    {
        try
        {
            Directory.CreateDirectory(AppSettings.DataFolder);
            string tmp = FilePath + ".tmp";
            File.WriteAllText(tmp, JsonSerializer.Serialize(list, Json));
            File.Move(tmp, FilePath, true);
        }
        catch { }
    }
}
