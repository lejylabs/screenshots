# Windows user autostart

```powershell
screenshots autostart status
screenshots autostart enable
screenshots autostart disable
```

These explicit commands manage only the `Screenshots` value under
`HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`. They do not
start the application, open a window, capture pixels or require administrator
privileges. `enable` registers the canonical absolute path of the invoking
executable, quoted as one command with no arguments. The normal Windows launch
starts the existing resident tray/hotkey application; it does not request a
window with `--show`. Login processing and Startup Apps policy remain controlled
by Windows. Microsoft documents that [Run entries can be delayed](https://learn.microsoft.com/en-us/windows/win32/setupapi/run-and-runonce-registry-keys),
so registration does not guarantee immediate or successful login-time startup.

Successful stdout is one JSON object:

```json
{"scope":"managed_user_entry","status":"enabled","registry_key":"HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Run","value_name":"Screenshots"}
```

`status` is `disabled` when the key or value is absent, `enabled` only when the
value is exactly the expected `REG_SZ` command for this executable, and
`conflict` for another command, registry type, malformed data or a value over
16 KiB. Errors use stderr and exit status 1 with no success JSON. These states
cover this value only: Startup Apps policy, other registry entries, Startup
folders, scheduled tasks, services or another installation are not inspected.
`disabled` therefore does not claim that Screenshots cannot start at login.

`enable` is idempotent for the exact command. It refuses to overwrite an edited
or foreign value. `disable` removes only the exact command and is idempotent
when absent. Neither command deletes the containing key or changes other values.
Moving the executable or using a different installation produces a conflict;
inspect and remove the old entry manually before enabling the new path. The
command does not infer ownership from a product name alone.

The registered value is `REG_SZ`, not an expandable string. Percent signs,
dollar signs and other shell characters in the executable path are literal;
there is no `cmd.exe`, PowerShell, shell evaluation or subprocess in the
management path. A canonical Windows extended path prefix is retained. Quotes,
control characters, NUL and a trailing directory separator are rejected. The
entire quoted command including its terminating NUL must fit 260 UTF-16 units,
a conservative interpretation of Microsoft's documented Run command limit.
The executable path is not saved in a separate application settings file.

Registry reads first check type and size, then read at most 16 KiB once, with
exact comparison including the UTF-16 terminator. [RegQueryValueEx does not
guarantee terminated strings](https://learn.microsoft.com/en-us/windows/win32/api/winreg/nf-winreg-regqueryvalueexw),
so unknown values are never interpreted or executed. Opening for status or an
already absent disable does not create a key. Enable requests only query/set
rights; an absent Run key can be created through the native registry API.
Owned handles close at command completion. No enumeration of unrelated entries,
new dependency, watcher, timer, background thread, service or network runtime is
added. Resource cost is one key handle and a bounded value buffer while running.

Native registry set/delete does not provide compare-and-swap here. The value is
rechecked immediately before mutation, so observed edits cause refusal, but a
concurrent writer can still race the final write or delete. Avoid editing this
same value concurrently. Registry policy, access failures and native stalls are
reported through existing synchronous CLI errors; no transaction or hard native
call deadline is claimed. A completed mutation followed by stdout failure reports
that the action completed and asks you to inspect status.

The actual module was cross-compiled in an isolated release test crate and three
fixtures passed under Wine: quoting/rejection, CreateProcessW launching only the
test executable's `--list` through its canonical quoted path, and native registry
enable/status/disable plus foreign/type/size/embedded-NUL conflicts. Registry
tests use a unique `HKCU\Software\ScreenshotsTest\UUID` child key, remove it after
completion or child failure, and preserve an unrelated value. They never write
the real Run key. Wine checks do not establish native Windows login, Startup Apps
policy, deployment-path or resource/performance acceptance; those remain manual
checks.
