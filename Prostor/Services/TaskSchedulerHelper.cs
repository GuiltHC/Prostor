using System.Security;
using System.Security.Principal;

namespace Prostor.Services;

public sealed class ScheduledTaskInfo
{
    public string Path { get; init; }
    public string Name { get; init; }
    public bool Enabled { get; init; }
    public string Command { get; init; }
    public string Arguments { get; init; }
    public string Author { get; init; }
    public string Description { get; init; }
    public bool AtLogon { get; init; }
    public bool AtBoot { get; init; }
    public string UserId { get; init; }
}

/// <summary>Thin wrapper over the Task Scheduler COM API ("Schedule.Service").</summary>
public static class TaskSchedulerHelper
{
    public const string Folder = @"\Prostor";

    private const int TASK_CREATE_OR_UPDATE = 6;
    private const int TASK_LOGON_INTERACTIVE_TOKEN = 3;
    private const int TASK_ENUM_HIDDEN = 1;
    private const int TASK_TRIGGER_BOOT = 8;
    private const int TASK_TRIGGER_LOGON = 9;
    private const int TASK_ACTION_EXEC = 0;

    private static dynamic Connect()
    {
        var type = Type.GetTypeFromProgID("Schedule.Service") ?? throw new InvalidOperationException("Планировщик заданий недоступен");
        dynamic svc = Activator.CreateInstance(type)!;
        svc.Connect();
        return svc;
    }

    public static string CurrentUser => WindowsIdentity.GetCurrent().Name;

    /// <summary>Creates or replaces a task from Task Scheduler XML. <paramref name="path"/> is like "\Prostor\AutoClean".</summary>
    public static void Register(string path, string xml)
    {
        dynamic svc = Connect();
        string folderPath = path.Substring(0, path.LastIndexOf('\\'));
        string name = path.Substring(path.LastIndexOf('\\') + 1);
        dynamic folder = GetOrCreateFolder(svc, string.IsNullOrEmpty(folderPath) ? "\\" : folderPath);
        folder.RegisterTask(name, xml, TASK_CREATE_OR_UPDATE, null, null, TASK_LOGON_INTERACTIVE_TOKEN, null);
    }

    public static bool Exists(string path)
    {
        try
        {
            dynamic svc = Connect();
            dynamic task = svc.GetFolder("\\").GetTask(path);
            return task != null;
        }
        catch
        {
            return false;
        }
    }

    public static void Delete(string path)
    {
        try
        {
            dynamic svc = Connect();
            string folderPath = path.Substring(0, path.LastIndexOf('\\'));
            dynamic folder = svc.GetFolder(string.IsNullOrEmpty(folderPath) ? "\\" : folderPath);
            folder.DeleteTask(path.Substring(path.LastIndexOf('\\') + 1), 0);
        }
        catch { }
    }

    public static void SetEnabled(string path, bool enabled)
    {
        dynamic svc = Connect();
        dynamic task = svc.GetFolder("\\").GetTask(path);
        task.Enabled = enabled;
    }

    /// <summary>The task's XML definition (used as a backup before deleting it).</summary>
    public static string ExportXml(string path)
    {
        dynamic svc = Connect();
        dynamic task = svc.GetFolder("\\").GetTask(path);
        return (string)task.Xml;
    }

    public static void ImportXml(string path, string xml)
    {
        dynamic svc = Connect();
        string folderPath = path.Substring(0, path.LastIndexOf('\\'));
        dynamic folder = GetOrCreateFolder(svc, string.IsNullOrEmpty(folderPath) ? "\\" : folderPath);
        folder.RegisterTask(path.Substring(path.LastIndexOf('\\') + 1), xml, TASK_CREATE_OR_UPDATE, null, null, TASK_LOGON_INTERACTIVE_TOKEN, null);
    }

    /// <summary>All tasks except the ones in \Microsoft\ (those belong to Windows).</summary>
    public static List<ScheduledTaskInfo> ListAll(bool includeMicrosoft = false)
    {
        var result = new List<ScheduledTaskInfo>();
        dynamic svc = Connect();
        Walk(svc.GetFolder("\\"), result, includeMicrosoft);
        return result;
    }

    private static void Walk(dynamic folder, List<ScheduledTaskInfo> result, bool includeMicrosoft)
    {
        string folderPath = (string)folder.Path;
        if (!includeMicrosoft && folderPath.StartsWith(@"\Microsoft", StringComparison.OrdinalIgnoreCase)) return;
        try
        {
            foreach (dynamic task in folder.GetTasks(TASK_ENUM_HIDDEN))
            {
                try
                {
                    result.Add(Describe(task));
                }
                catch { }
            }
        }
        catch { }
        try
        {
            foreach (dynamic sub in folder.GetFolders(0)) Walk(sub, result, includeMicrosoft);
        }
        catch { }
    }

    private static ScheduledTaskInfo Describe(dynamic task)
    {
        dynamic def = task.Definition;
        string command = null, arguments = null;
        foreach (dynamic action in def.Actions)
        {
            if ((int)action.Type != TASK_ACTION_EXEC) continue;
            command = Environment.ExpandEnvironmentVariables(((string)action.Path ?? "").Trim('"'));
            arguments = (string)action.Arguments;
            break;
        }
        bool logon = false, boot = false;
        foreach (dynamic trigger in def.Triggers)
        {
            int t = (int)trigger.Type;
            if (t == TASK_TRIGGER_LOGON) logon = true;
            if (t == TASK_TRIGGER_BOOT) boot = true;
        }
        string author = null, description = null, user = null;
        try { author = (string)def.RegistrationInfo.Author; } catch { }
        try { description = (string)def.RegistrationInfo.Description; } catch { }
        try { user = (string)def.Principal.UserId; } catch { }
        return new ScheduledTaskInfo
        {
            Path = (string)task.Path,
            Name = (string)task.Name,
            Enabled = (bool)task.Enabled,
            Command = command,
            Arguments = arguments,
            Author = author,
            Description = description,
            AtLogon = logon,
            AtBoot = boot,
            UserId = user
        };
    }

    private static dynamic GetOrCreateFolder(dynamic svc, string path)
    {
        try
        {
            return svc.GetFolder(path);
        }
        catch
        {
            return svc.GetFolder("\\").CreateFolder(path.TrimStart('\\'), null);
        }
    }

    // ------------------------------------------------------------------ XML builders

    public static string Escape(string s) => SecurityElement.Escape(s ?? "");

    private const string SettingsXml = """
          <Settings>
            <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
            <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
            <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
            <StartWhenAvailable>true</StartWhenAvailable>
            <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
            <IdleSettings><StopOnIdleEnd>false</StopOnIdleEnd><RestartOnIdle>false</RestartOnIdle></IdleSettings>
            <AllowStartOnDemand>true</AllowStartOnDemand>
            <Enabled>true</Enabled>
            <ExecutionTimeLimit>PT2H</ExecutionTimeLimit>
          </Settings>
        """;

    private static string Wrap(string description, string trigger, string command, string arguments) => $"""
        <?xml version="1.0" encoding="UTF-16"?>
        <Task version="1.3" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
          <RegistrationInfo><Author>Простор</Author><Description>{Escape(description)}</Description></RegistrationInfo>
          <Triggers>{trigger}</Triggers>
          <Principals>
            <Principal id="Author"><UserId>{Escape(CurrentUser)}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal>
          </Principals>
        {SettingsXml}
          <Actions Context="Author"><Exec><Command>{Escape(command)}</Command><Arguments>{Escape(arguments)}</Arguments></Exec></Actions>
        </Task>
        """;

    /// <summary>Weekly (Sunday) or monthly (1st day) run at 12:00; missed runs start as soon as the PC is on.</summary>
    public static string CalendarTaskXml(string description, bool monthly, string command, string arguments)
    {
        string start = DateTime.Today.AddDays(1).ToString("yyyy-MM-dd") + "T12:00:00";
        string schedule = monthly
            ? "<ScheduleByMonth><DaysOfMonth><Day>1</Day></DaysOfMonth><Months><January/><February/><March/><April/><May/><June/><July/><August/><September/><October/><November/><December/></Months></ScheduleByMonth>"
            : "<ScheduleByWeek><WeeksInterval>1</WeeksInterval><DaysOfWeek><Sunday/></DaysOfWeek></ScheduleByWeek>";
        return Wrap(description, $"<CalendarTrigger><StartBoundary>{start}</StartBoundary><Enabled>true</Enabled>{schedule}</CalendarTrigger>",
            command, arguments);
    }

    /// <summary>Runs at the user's logon after a delay such as "PT1M".</summary>
    public static string LogonTaskXml(string description, string delay, string command, string arguments) =>
        Wrap(description, $"<LogonTrigger><Enabled>true</Enabled><UserId>{Escape(CurrentUser)}</UserId><Delay>{delay}</Delay></LogonTrigger>",
            command, arguments);
}
