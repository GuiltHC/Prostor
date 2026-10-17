using Prostor.Core;

namespace Prostor.Models;

public sealed class CleanTarget
{
    public string Path { get; init; }
    /// <summary>File name mask applied to files (e.g. "thumbcache_*.db").</summary>
    public string Pattern { get; init; } = "*";
    public bool Recursive { get; init; } = true;
    /// <summary>Only files older than this are removed (protects files in active use).</summary>
    public TimeSpan MinAge { get; init; } = TimeSpan.Zero;
    /// <summary>Remove sub-folders that became empty (the target folder itself is kept).</summary>
    public bool RemoveEmptyDirs { get; init; } = true;
    /// <summary>The target is a single file rather than a folder.</summary>
    public bool IsFile { get; init; }
}

public sealed class CleanResult
{
    public long FreedBytes;
    public long DeletedFiles;
    public long FailedFiles;
}

public sealed class CleanupItem : ObservableObject
{
    /// <summary>Returned by <see cref="CustomMeasure"/> when the size is only known after cleaning.</summary>
    public const long UnknownSize = -2;

    public string Id { get; init; }
    public string Group { get; init; }
    public string Title { get; init; }
    public string Description { get; init; }
    public string Glyph { get; init; }
    public List<CleanTarget> Targets { get; init; } = new();
    public bool RequiresAdmin { get; init; }
    public bool IsRecycleBin { get; init; }

    /// <summary>Items that are not plain folders (system tools: DISM, powercfg, pnputil, shadow copies).</summary>
    public Func<CancellationToken, (long size, long count)> CustomMeasure { get; init; }
    public Func<CancellationToken, CleanResult> CustomClean { get; init; }
    /// <summary>Takes minutes rather than seconds.</summary>
    public bool IsSlow { get; init; }
    /// <summary>The size is an estimate.</summary>
    public bool IsEstimate { get; init; }
    /// <summary>A side effect the user should know about before cleaning.</summary>
    public string Warning { get; init; }
    /// <summary>Word forms (one, few, many) for <see cref="CountText"/> instead of "N файлов".</summary>
    public string[] CountForms { get; init; }

    public bool IsLocked => RequiresAdmin && !ShellHelper.IsAdmin;

    private bool _isChecked;
    public bool IsChecked { get => _isChecked; set => Set(ref _isChecked, value); }

    private long _size = -1;
    public long Size
    {
        get => _size;
        set
        {
            if (Set(ref _size, value))
            {
                OnPropertyChanged(nameof(SizeText));
                OnPropertyChanged(nameof(IsEmpty));
                OnPropertyChanged(nameof(IsCleanable));
            }
        }
    }

    private bool _sizeUnknown;
    public bool SizeUnknown
    {
        get => _sizeUnknown;
        set
        {
            if (!Set(ref _sizeUnknown, value)) return;
            OnPropertyChanged(nameof(SizeText));
            OnPropertyChanged(nameof(IsEmpty));
            OnPropertyChanged(nameof(IsCleanable));
        }
    }

    private long _fileCount;
    public long FileCount
    {
        get => _fileCount;
        set
        {
            if (Set(ref _fileCount, value)) OnPropertyChanged(nameof(CountText));
        }
    }

    private bool _isScanning;
    public bool IsScanning { get => _isScanning; set => Set(ref _isScanning, value); }

    public bool IsEmpty => _size == 0 && !_sizeUnknown;
    public bool IsCleanable => !IsLocked && (_size > 0 || _sizeUnknown);

    public string SizeText => _sizeUnknown ? "Узнаем после" : _size < 0 ? "" : _size == 0 ? "Чисто" : (IsEstimate ? "≈ " : "") + Fmt.Size(_size);

    public string CountText => IsRecycleBin
        ? Fmt.Count(_fileCount, "объект", "объекта", "объектов")
        : CountForms != null
            ? _fileCount > 0 ? Fmt.Count(_fileCount, CountForms[0], CountForms[1], CountForms[2]) : ""
            : _fileCount > 0 ? Fmt.Files(_fileCount) : "";
}

public sealed class CleanupGroup
{
    public string Title { get; init; }
    public string Hint { get; init; }
    public List<CleanupItem> Items { get; init; }
}
