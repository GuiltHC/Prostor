using System.Collections.ObjectModel;
using System.Windows.Input;
using Prostor.Core;
using Prostor.Models;

namespace Prostor.ViewModels;

/// <summary>In-window modal dialog (confirmation, progress or checklist).</summary>
public sealed class DialogViewModel : ObservableObject
{
    private readonly TaskCompletionSource<bool> _tcs = new();

    public DialogViewModel()
    {
        PrimaryCommand = new RelayCommand(() => Close(true, "primary"), () => PrimaryEnabled);
        SecondaryCommand = new RelayCommand(() => Close(false, "secondary"));
        TertiaryCommand = new RelayCommand(() => Close(false, "tertiary"));
    }

    /// <summary>Which button closed the dialog: "primary", "secondary", "tertiary" (null when closed programmatically).</summary>
    public string Choice { get; private set; }

    private string _tertiaryText;
    public string TertiaryText { get => _tertiaryText; set => Set(ref _tertiaryText, value); }
    public ICommand TertiaryCommand { get; }

    // ---- progress
    private bool _showProgress;
    public bool ShowProgress { get => _showProgress; set => Set(ref _showProgress, value); }

    private bool _isIndeterminate = true;
    public bool IsIndeterminate { get => _isIndeterminate; set => Set(ref _isIndeterminate, value); }

    private double _progress;
    public double Progress { get => _progress; set => Set(ref _progress, value); }

    private string _progressText;
    public string ProgressText { get => _progressText; set => Set(ref _progressText, value); }

    private bool _showOverall;
    public bool ShowOverall { get => _showOverall; set => Set(ref _showOverall, value); }

    private double _overallProgress;
    public double OverallProgress { get => _overallProgress; set => Set(ref _overallProgress, value); }

    private string _overallText;
    public string OverallText { get => _overallText; set => Set(ref _overallText, value); }

    private string _title;
    public string Title { get => _title; set => Set(ref _title, value); }

    private string _message;
    public string Message { get => _message; set => Set(ref _message, value); }

    public string Glyph { get; init; } = "";
    /// <summary>"accent", "danger", "success" or "warning".</summary>
    public string Kind { get; init; } = "accent";

    private string _primaryText = "OK";
    public string PrimaryText { get => _primaryText; set => Set(ref _primaryText, value); }

    private string _secondaryText;
    public string SecondaryText { get => _secondaryText; set => Set(ref _secondaryText, value); }

    public bool IsDanger { get; init; }

    private bool _primaryEnabled = true;
    public bool PrimaryEnabled { get => _primaryEnabled; set => Set(ref _primaryEnabled, value); }

    public string CheckText { get; init; }
    private bool _checkValue;
    public bool CheckValue { get => _checkValue; set => Set(ref _checkValue, value); }

    public string Check2Text { get; init; }
    private bool _check2Value;
    public bool Check2Value { get => _check2Value; set => Set(ref _check2Value, value); }

    public string Check3Text { get; init; }
    public bool Check3Enabled { get; init; } = true;
    private bool _check3Value;
    public bool Check3Value { get => _check3Value; set => Set(ref _check3Value, value); }

    /// <summary>Optional small print under the checkboxes.</summary>
    public string Footnote { get; init; }

    public ObservableCollection<SelectableItem> Items { get; init; }
    public bool HasItems => Items is { Count: > 0 };

    private bool _isBusy;
    public bool IsBusy { get => _isBusy; set => Set(ref _isBusy, value); }

    private string _detail;
    public string Detail { get => _detail; set => Set(ref _detail, value); }

    public ICommand PrimaryCommand { get; }
    public ICommand SecondaryCommand { get; }

    public Task<bool> Result => _tcs.Task;

    public void Close(bool result) => Close(result, null);

    public void Close(bool result, string choice)
    {
        if (_tcs.Task.IsCompleted) return;
        Choice ??= choice;
        var main = MainViewModel.Instance;
        if (main != null && main.Dialog == this) main.Dialog = null;
        _tcs.TrySetResult(result);
    }
}

public sealed class ToastInfo
{
    public string Text { get; init; }
    public string Glyph { get; init; }
    public string Kind { get; init; }
}
