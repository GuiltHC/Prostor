using System.Windows;
using System.Windows.Threading;
using Prostor.Core;
using Prostor.Services;

namespace Prostor;

public partial class App : Application
{
    public static event Action ThemeChanged
    {
        add => ThemeManager.Changed += value;
        remove => ThemeManager.Changed -= value;
    }

    public static bool IsDark => ThemeManager.IsDark;

    protected override void OnStartup(StartupEventArgs e)
    {
        base.OnStartup(e);
        DispatcherUnhandledException += OnUnhandled;
        TaskScheduler.UnobservedTaskException += (_, ex) => ex.SetObserved();

        AppSettings.Load();
        Updater.CleanupAfterUpdate();
        var args = e.Args;

        if (args.Contains("--auto-clean"))
        {
            ThemeManager.Apply();
            ShutdownMode = ShutdownMode.OnExplicitShutdown;
            _ = AutoCleanRunner.RunAsync();
            return;
        }

        bool newWindow = args.Contains("--new-window");
        bool first = SingleInstance.TryAcquire(newWindow ? 4000 : 0);
        if (!first && !newWindow && SingleInstance.SendToRunning(args.Length == 0 ? new[] { "--activate" } : args))
        {
            Shutdown();
            return;
        }

        ThemeManager.Apply();
        var window = new MainWindow { StartupArgs = args };
        MainWindow = window;
        if (first)
            SingleInstance.Listen(a => Dispatcher.InvokeAsync(() => window.HandleArgs(a, activate: true)));
        window.Show();
    }

    private static void OnUnhandled(object sender, DispatcherUnhandledExceptionEventArgs e)
    {
        e.Handled = true;
        Log(e.Exception);
        var vm = ViewModels.MainViewModel.Instance;
        if (vm != null) _ = vm.ShowError("Что-то пошло не так", e.Exception.Message);
        else MessageBox.Show(e.Exception.Message, "Простор", MessageBoxButton.OK, MessageBoxImage.Error);
    }

    public static void Log(Exception ex)
    {
        try
        {
            string log = Path.Combine(AppSettings.DataFolder, "error.log");
            Directory.CreateDirectory(AppSettings.DataFolder);
            if (File.Exists(log) && new FileInfo(log).Length > 1 << 20) File.Delete(log);
            File.AppendAllText(log, $"[{DateTime.Now:u}] {ex}\n\n");
        }
        catch { }
    }
}
