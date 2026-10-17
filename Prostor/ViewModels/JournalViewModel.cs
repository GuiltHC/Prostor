using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Windows;
using System.Windows.Data;
using System.Windows.Input;
using Prostor.Core;
using Prostor.Services;

namespace Prostor.ViewModels;

public enum JournalFilter { All, Programs, Cleanup, Files, System }

public sealed class JournalItem : ObservableObject
{
    public JournalEntry Entry { get; init; }

    public string Title => Entry.Title;
    public string Detail => Entry.Detail;
    public string TimeText => Entry.Time.ToString("HH:mm");
    public string DayText
    {
        get
        {
            var d = Entry.Time.Date;
            if (d == DateTime.Today) return "Сегодня";
            if (d == DateTime.Today.AddDays(-1)) return "Вчера";
            return d.ToString(d.Year == DateTime.Today.Year ? "d MMMM, dddd" : "d MMMM yyyy", Fmt.Ru);
        }
    }
    public string SizeText => Entry.Bytes > 0 ? Fmt.Size(Entry.Bytes) : null;

    public string Glyph => Entry.Kind switch
    {
        "uninstall" or "force" => "",
        "entry" => "",
        "leftovers" => "",
        "cleanup" or "autoclean" => "",
        "delete" or "duplicates" => "",
        "compress" => "",
        "move" => "",
        "startup" => "",
        "update" => "",
        _ => ""
    };

    public string Accent => Entry.Kind switch
    {
        "uninstall" or "force" or "delete" or "duplicates" => "danger",
        "cleanup" or "autoclean" or "compress" => "success",
        "startup" => "warning",
        _ => "accent"
    };

    public bool CanUndo => Entry.Undo != UndoKind.None && !Entry.Undone && !IsBusy;
    public bool IsUndone => Entry.Undone;
    public string UndoText => UndoService.Describe(Entry.Undo);

    private bool _isBusy;
    public bool IsBusy
    {
        get => _isBusy;
        set
        {
            if (Set(ref _isBusy, value)) OnPropertyChanged(nameof(CanUndo));
        }
    }

    public void RaiseUndone()
    {
        OnPropertyChanged(nameof(CanUndo));
        OnPropertyChanged(nameof(IsUndone));
    }
}

public sealed class JournalViewModel : ObservableObject
{
    private readonly MainViewModel _main;

    public ObservableCollection<JournalItem> Items { get; } = new();
    public ListCollectionView View { get; }

    public ICommand UndoCommand { get; }
    public ICommand OpenBackupsCommand { get; }
    public ICommand ClearCommand { get; }

    public JournalViewModel(MainViewModel main)
    {
        _main = main;
        View = new ListCollectionView(Items) { Filter = o => Matches((JournalItem)o) };
        View.GroupDescriptions.Add(new PropertyGroupDescription(nameof(JournalItem.DayText)));
        UndoCommand = new RelayCommand(async p =>
        {
            if (p is JournalItem { CanUndo: true } i) await UndoAsync(i);
        });
        OpenBackupsCommand = new RelayCommand(() =>
        {
            Directory.CreateDirectory(RegistryCleaner.BackupFolder);
            ShellHelper.OpenFolder(RegistryCleaner.BackupFolder);
        });
        ClearCommand = new RelayCommand(async () =>
        {
            var ok = await _main.ShowDialog(new DialogViewModel
            {
                Title = "Очистить журнал?",
                Message = "Записи исчезнут, и отменить эти действия отсюда будет нельзя. Резервные копии реестра останутся в папке Backups.",
                Glyph = "", Kind = "warning", PrimaryText = "Очистить", SecondaryText = "Отмена"
            });
            if (!ok) return;
            ActionJournal.Clear();
            Reload();
        }, () => Items.Count > 0);
        ActionJournal.Changed += () => Application.Current?.Dispatcher.InvokeAsync(() =>
        {
            if (_main.CurrentKey == "journal") Reload();
        });
    }

    private JournalFilter _filter;
    public JournalFilter Filter
    {
        get => _filter;
        set
        {
            if (Set(ref _filter, value)) View.Refresh();
        }
    }

    private bool Matches(JournalItem i) => _filter switch
    {
        JournalFilter.Programs => i.Entry.Kind is "uninstall" or "force" or "entry" or "leftovers" or "update",
        JournalFilter.Cleanup => i.Entry.Kind is "cleanup" or "autoclean",
        JournalFilter.Files => i.Entry.Kind is "delete" or "duplicates" or "compress" or "move",
        JournalFilter.System => i.Entry.Kind is "startup" or "system",
        _ => true
    };

    public bool IsEmpty => Items.Count == 0;

    private long _freedTotal;
    public string Summary => Items.Count == 0
        ? "Здесь появятся удалённые программы, очистки и другие изменения — и кнопки, чтобы их отменить"
        : $"{Fmt.Count(Items.Count, "запись", "записи", "записей")} · освобождено всего {Fmt.Size(_freedTotal)}";

    public void Reload()
    {
        var entries = ActionJournal.Load();
        Items.Clear();
        foreach (var e in entries) Items.Add(new JournalItem { Entry = e });
        _freedTotal = entries.Where(e => e.Kind is "uninstall" or "cleanup" or "autoclean" or "delete" or "duplicates" or "leftovers" or "force" or "compress" or "move")
            .Sum(e => Math.Max(0, e.Bytes));
        OnPropertyChanged(nameof(IsEmpty));
        OnPropertyChanged(nameof(Summary));
        CommandManager.InvalidateRequerySuggested();
    }

    private async Task UndoAsync(JournalItem item)
    {
        var e = item.Entry;
        var ok = await _main.ShowDialog(new DialogViewModel
        {
            Title = UndoService.Describe(e.Undo) + "?",
            Message = $"«{e.Title}»" + (e.Undo switch
            {
                UndoKind.RegImport => "\n\nЗапись программы вернётся в реестр из резервной копии.",
                UndoKind.RecycleRestore => "\n\nФайлы и папки вернутся из корзины на прежние места.",
                UndoKind.Decompress => "\n\nФайлы будут распакованы и снова займут исходный объём.",
                UndoKind.MoveBack => "\n\nПапка будет скопирована обратно, ссылка на новое место удалится.",
                UndoKind.Hibernation => "\n\nФайл гибернации снова займёт место на системном диске.",
                _ => ""
            }),
            Glyph = "",
            PrimaryText = "Отменить действие",
            SecondaryText = "Не нужно"
        });
        if (!ok) return;

        item.IsBusy = true;
        var busy = new DialogViewModel { Title = "Возвращаем…", Glyph = "", IsBusy = true, PrimaryText = null, ShowProgress = true };
        _ = _main.ShowDialog(busy);
        string error = await UndoService.UndoAsync(e, new Progress<string>(s => busy.ProgressText = s));
        busy.Close(true);
        item.IsBusy = false;
        if (error == null)
        {
            ActionJournal.MarkUndone(e.Id);
            e.Undone = true;
            item.RaiseUndone();
            _main.ShowToast("Готово — действие отменено");
            if (e.Undo == UndoKind.RegImport) _ = _main.Programs.LoadAsync();
            if (e.Undo == UndoKind.DelayedStart) _ = _main.Startup.LoadAsync();
        }
        else
        {
            await _main.ShowError("Не получилось отменить", error);
        }
    }
}
