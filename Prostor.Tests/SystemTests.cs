using Microsoft.Win32;
using Prostor.Core;
using Prostor.Models;
using Prostor.Services;
using Xunit;

namespace Prostor.Tests;

/// <summary>
/// Run the real features against the current user's registry, folders, Task Scheduler and Recycle Bin, using dummy
/// entries that every test removes again. They change the system, so they only run on request:
/// <c>Prostor.Tests.exe -class Prostor.Tests.SystemTests -explicit only</c>.
/// </summary>
public class SystemTests
{
    private const string AppName = "Prostor Dummy Tool";
    private const string Vendor = "Prostor Test Vendor";
    private const string UninstallKey = @"Software\Microsoft\Windows\CurrentVersion\Uninstall\ProstorDummyTool";
    private const string RunKey = @"Software\Microsoft\Windows\CurrentVersion\Run";
    private const string ApprovedRunKey = @"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

    private static string Local => Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);

    [Fact(Explicit = true)]
    public async Task ForcedUninstall_FindsLeftoversRemovesThemAndUndoRestoresEverything()
    {
        string install = Path.Combine(Local, "Programs", AppName);
        string vendorData = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), Vendor);
        string data = Path.Combine(vendorData, AppName);
        string shortcut = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.Programs), AppName + ".lnk");
        string exe = Path.Combine(install, "dummy.exe");
        const string task = @"\ProstorDummyTool Updater";
        const string runValue = "ProstorDummyTool";
        var backups = new List<string>();

        try
        {
            // a program whose uninstaller is gone, with everything such programs usually leave behind
            Directory.CreateDirectory(install);
            File.WriteAllText(Path.Combine(install, "settings.ini"), "x");
            Directory.CreateDirectory(data);
            File.WriteAllText(Path.Combine(data, "cache.bin"), new string('x', 10_000));
            using (var k = Registry.CurrentUser.CreateSubKey(UninstallKey))
            {
                k.SetValue("DisplayName", AppName + " 1.2.3 (x64)");
                k.SetValue("Publisher", Vendor + ", Inc.");
                k.SetValue("DisplayVersion", "1.2.3");
                k.SetValue("InstallLocation", install);
                k.SetValue("UninstallString", $"\"{Path.Combine(install, "unins000.exe")}\"");
            }
            using (var k = Registry.CurrentUser.CreateSubKey($@"Software\{Vendor}\{AppName}")) k.SetValue("Theme", "dark");
            using (var k = Registry.CurrentUser.CreateSubKey(RunKey)) k.SetValue(runValue, $"\"{exe}\" --background");
            TaskSchedulerHelper.Register(task, TaskSchedulerHelper.CalendarTaskXml("dummy", false, exe, ""));
            dynamic shell = Activator.CreateInstance(Type.GetTypeFromProgID("WScript.Shell")!);
            var lnk = shell.CreateShortcut(shortcut);
            lnk.TargetPath = exe;
            lnk.Save();

            var all = AppInventory.LoadDesktop();
            var app = Assert.Single(all, a => a.Name.StartsWith(AppName));
            Assert.True(Uninstaller.IsUninstallerMissing(app));

            string entryBackup = RegistryCleaner.RemoveUninstallEntry(app);
            Assert.NotNull(entryBackup);
            backups.Add(entryBackup);
            Assert.Null(Registry.CurrentUser.OpenSubKey(UninstallKey));

            var found = LeftoverFinder.Find(app, all);
            string Describe() => string.Join("\n", found.Select(f => $"{f.Kind}: {f.Target} {f.ValueName}"));
            // nothing that belongs to anything else
            Assert.All(found, f => Assert.True(
                (f.Target + f.ValueName).Contains("Prostor Dummy Tool") || (f.Target + f.ValueName).Contains("ProstorDummyTool") ||
                f.Target.Contains(Vendor), Describe()));
            Assert.Contains(found, f => f.Kind == LeftoverKind.Folder && f.Target == install);
            Assert.Contains(found, f => f.Kind == LeftoverKind.Folder && f.Target == data);
            Assert.Contains(found, f => f.Kind == LeftoverKind.Shortcut && f.Target == shortcut);
            Assert.Contains(found, f => f.Kind == LeftoverKind.RegistryKey && f.Target == $@"HKEY_CURRENT_USER\Software\{Vendor}");
            Assert.Contains(found, f => f.Kind == LeftoverKind.StartupEntry && f.ValueName == runValue);
            Assert.Contains(found, f => f.Kind == LeftoverKind.Task && f.Target == task);
            Assert.Equal(6, found.Count);

            var removal = LeftoverFinder.Remove(found, "prostor-test");
            backups.AddRange(removal.RegBackups);
            backups.AddRange(removal.TaskBackups.Select(t => t.Split('|')[1]));
            Assert.Equal(0, removal.Failed);
            Assert.False(Directory.Exists(install));
            Assert.False(Directory.Exists(data));
            Assert.False(File.Exists(shortcut));
            Assert.Null(Registry.CurrentUser.OpenSubKey($@"Software\{Vendor}"));
            using (var k = Registry.CurrentUser.OpenSubKey(RunKey)) Assert.Null(k!.GetValue(runValue));
            Assert.False(TaskSchedulerHelper.Exists(task));

            // the three journal entries the Programs page writes, undone the way the Journal page does it
            Assert.Null(await UndoService.UndoAsync(new JournalEntry { Undo = UndoKind.RecycleRestore, UndoData = removal.Recycled }));
            Assert.Null(await UndoService.UndoAsync(new JournalEntry { Undo = UndoKind.RegImport, UndoData = removal.RegBackups.Append(entryBackup).ToList() }));
            Assert.Null(await UndoService.UndoAsync(new JournalEntry { Undo = UndoKind.TaskRestore, UndoData = removal.TaskBackups }));
            Assert.True(File.Exists(Path.Combine(install, "settings.ini")));
            Assert.True(File.Exists(Path.Combine(data, "cache.bin")));
            Assert.True(File.Exists(shortcut));
            using (var k = Registry.CurrentUser.OpenSubKey($@"Software\{Vendor}\{AppName}")) Assert.Equal("dark", k?.GetValue("Theme"));
            using (var k = Registry.CurrentUser.OpenSubKey(RunKey)) Assert.NotNull(k!.GetValue(runValue));
            Assert.NotNull(Registry.CurrentUser.OpenSubKey(UninstallKey));
            Assert.True(TaskSchedulerHelper.Exists(task));
        }
        finally
        {
            TryRun(() => Directory.Delete(install, true));
            TryRun(() => Directory.Delete(vendorData, true));
            TryRun(() => File.Delete(shortcut));
            TryRun(() => Registry.CurrentUser.DeleteSubKeyTree(UninstallKey, false));
            TryRun(() => Registry.CurrentUser.DeleteSubKeyTree($@"Software\{Vendor}", false));
            TryRun(() => { using var k = Registry.CurrentUser.OpenSubKey(RunKey, true); k?.DeleteValue(runValue, false); });
            TryRun(() => TaskSchedulerHelper.Delete(task));
            foreach (var b in backups) TryRun(() => File.Delete(b));
        }
    }

    [Fact(Explicit = true)]
    public async Task DelayedStart_MovesTheEntryToATaskAndUndoBringsItBack()
    {
        const string name = "ProstorDelayTest";
        string cmd = $"\"{Path.Combine(Environment.SystemDirectory, "cmd.exe")}\" /c exit";
        string taskPath = null;
        try
        {
            using (var k = Registry.CurrentUser.CreateSubKey(RunKey)) k.SetValue(name, cmd);
            var item = Assert.Single(StartupService.Load(), i => i.Kind == StartupKind.Run && i.Name == name);
            Assert.True(item.IsEnabled);

            taskPath = StartupService.MakeDelayed(item);
            Assert.True(TaskSchedulerHelper.Exists(taskPath));
            Assert.Contains("PT1M", TaskSchedulerHelper.ExportXml(taskPath));
            Assert.Equal(3, ApprovedState(name)); // the entry itself is off…
            var reloaded = Assert.Single(StartupService.Load(), i => i.Kind == StartupKind.Run && i.Name == name);
            Assert.Equal(taskPath, reloaded.DelayTaskPath); // …and the page shows it as delayed, not disabled
            Assert.True(reloaded.IsEnabled);

            Assert.Null(await UndoService.UndoAsync(new JournalEntry { Undo = UndoKind.DelayedStart, UndoData = { item.Key, taskPath } }));
            Assert.False(TaskSchedulerHelper.Exists(taskPath));
            Assert.Equal(2, ApprovedState(name));
        }
        finally
        {
            TryRun(() => { using var k = Registry.CurrentUser.OpenSubKey(RunKey, true); k?.DeleteValue(name, false); });
            TryRun(() => { using var k = Registry.CurrentUser.OpenSubKey(ApprovedRunKey, true); k?.DeleteValue(name, false); });
            if (taskPath != null) TryRun(() => TaskSchedulerHelper.Delete(taskPath));
        }
    }

    [Fact(Explicit = true)]
    public async Task Compression_ShrinksTheFolderAndUndoUnpacksIt()
    {
        string dir = Path.Combine(Path.GetTempPath(), "prostor-it-compress-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(dir);
        try
        {
            var ct = TestContext.Current.CancellationToken;
            string text = string.Concat(Enumerable.Repeat("Простор сжимает папки средствами NTFS. ", 8000));
            for (int i = 0; i < 10; i++) File.WriteAllText(Path.Combine(dir, $"log{i}.txt"), text);
            long logical = Directory.EnumerateFiles(dir).Sum(f => new FileInfo(f).Length);
            Assert.True(Compression.IsSupported(dir));
            Assert.True(Compression.PhysicalSize(dir, ct) >= logical * 0.9);

            await Compression.CompressAsync(dir, null, ct);
            Assert.True(Compression.PhysicalSize(dir, ct) < logical / 3, $"{Compression.PhysicalSize(dir, ct)} of {logical}");
            Assert.Equal(text, File.ReadAllText(Path.Combine(dir, "log3.txt")));

            Assert.Null(await UndoService.UndoAsync(new JournalEntry { Undo = UndoKind.Decompress, UndoData = { dir } }));
            Assert.True(Compression.PhysicalSize(dir, ct) >= logical * 0.9);
        }
        finally
        {
            TryRun(() => Directory.Delete(dir, true));
        }
    }

    [Fact(Explicit = true)]
    public async Task FolderMove_LeavesAJunctionAndUndoMovesItBack()
    {
        string root = Path.Combine(Path.GetTempPath(), "prostor-it-move-" + Guid.NewGuid().ToString("N"));
        string source = Path.Combine(root, "src", "Data");
        string targetParent = Path.Combine(root, "dst");
        Directory.CreateDirectory(Path.Combine(source, "sub"));
        Directory.CreateDirectory(targetParent);
        try
        {
            File.WriteAllText(Path.Combine(source, "a.txt"), "first");
            File.WriteAllBytes(Path.Combine(source, "sub", "b.bin"), new byte[200_000]);
            Assert.Null(FolderMover.CheckCanMove(source));

            string target = await FolderMover.MoveAsync(source, targetParent, null, TestContext.Current.CancellationToken);
            Assert.Equal(Path.Combine(targetParent, "Data"), target);
            Assert.True(new DirectoryInfo(source).Attributes.HasFlag(FileAttributes.ReparsePoint));
            Assert.Equal("first", File.ReadAllText(Path.Combine(source, "a.txt"))); // programs still find their files
            Assert.True(File.Exists(Path.Combine(target, "sub", "b.bin")));
            Assert.False(Directory.Exists(source + ".prostor-old"));

            Assert.Null(await UndoService.UndoAsync(new JournalEntry { Undo = UndoKind.MoveBack, UndoData = { source, target } }));
            Assert.False(new DirectoryInfo(source).Attributes.HasFlag(FileAttributes.ReparsePoint));
            Assert.Equal(200_000, new FileInfo(Path.Combine(source, "sub", "b.bin")).Length);
            Assert.False(Directory.Exists(target));
        }
        finally
        {
            TryRun(() => { if (new DirectoryInfo(source).Attributes.HasFlag(FileAttributes.ReparsePoint)) Directory.Delete(source); });
            TryRun(() => Directory.Delete(root, true));
        }
    }

    [Fact(Explicit = true)]
    public void ExplorerMenu_IsAddedForTheCurrentUserAndRemoved()
    {
        if (ExplorerIntegration.IsEnabled) Assert.Skip("The menu is already on; the test would replace the user's command.");
        try
        {
            ExplorerIntegration.Enable();
            Assert.True(ExplorerIntegration.IsEnabled);
            Assert.False(ExplorerIntegration.PointsElsewhere);
            foreach (var root in new[] { "Directory", @"Directory\Background", "Drive" })
            {
                using var k = Registry.CurrentUser.OpenSubKey($@"Software\Classes\{root}\shell\Prostor.Analyze\command");
                Assert.Contains("--scan", k?.GetValue(null) as string);
            }
            ExplorerIntegration.Disable();
            Assert.False(ExplorerIntegration.IsEnabled);
            Assert.Null(Registry.CurrentUser.OpenSubKey(@"Software\Classes\Drive\shell\Prostor.Analyze"));
        }
        finally
        {
            ExplorerIntegration.Disable();
        }
    }

    [Fact(Explicit = true)]
    public void AutoClean_TaskIsRegisteredAndRemoved()
    {
        if (AutoCleanScheduler.IsRegistered) Assert.Skip("Scheduled cleanup is already on; the test would replace it.");
        try
        {
            AutoCleanScheduler.Apply("monthly");
            Assert.True(AutoCleanScheduler.IsRegistered);
            string xml = TaskSchedulerHelper.ExportXml(AutoCleanScheduler.TaskPath);
            Assert.Contains("--auto-clean", xml);
            Assert.Contains("ScheduleByMonth", xml);
            Assert.False(AutoCleanScheduler.PointsElsewhere);
            AutoCleanScheduler.Apply("off");
            Assert.False(AutoCleanScheduler.IsRegistered);
        }
        finally
        {
            AutoCleanScheduler.Apply("off");
        }
    }

    private static int ApprovedState(string name)
    {
        using var k = Registry.CurrentUser.OpenSubKey(ApprovedRunKey);
        return k?.GetValue(name) is byte[] { Length: > 0 } b ? b[0] : 0;
    }

    private static void TryRun(Action a)
    {
        try { a(); }
        catch { }
    }
}
