# macOS user autostart

```sh
screenshots autostart status
screenshots autostart enable
screenshots autostart disable
```

These explicit commands manage only
`$HOME/Library/LaunchAgents/me.lejy.screenshots.plist`. `HOME` must be absolute.
They do not run `launchctl`, load or unload an agent, start a process, open a
window or capture pixels. Enable creates a LaunchAgent property list for a
subsequent login, with `Label=me.lejy.screenshots`, `RunAtLoad=true` and
`ProgramArguments` containing the canonical absolute current executable path and
`--show`. The requested startup opens the normal application window; no hidden
resident mode, persistent worker or automatic restart is introduced.

Apple documents the user `Library/LaunchAgents` directory and tokenized
`ProgramArguments` in its [launchd guide](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html).
This command writes the file only. A loaded agent or an already running process
is unaffected by disable until the login session changes; macOS login policy may
also affect whether or when the entry launches. Native login, signing,
quarantine and permission acceptance remain unverified by file tests.

Successful stdout is one JSON object:

```json
{"scope":"managed_user_entry","status":"enabled","path":"/Users/user/Library/LaunchAgents/me.lejy.screenshots.plist","path_encoding":"lossy-native"}
```

The local file state is `disabled` when absent, `enabled` when every byte matches
the generated entry for this executable, or `conflict` for a foreign, edited,
oversized, symlink or non-regular entry. Errors use stderr and exit status 1
without success JSON. These states do not query launchd, Login Items, system
LaunchAgents, other installations or loaded job state. `disabled` means that
this managed file is absent, not that no other mechanism can start the app.

Enable and disable are idempotent for their existing desired state. Enable
never overwrites a file, and disable only removes an exact owned entry. A plist
edited by the user or pointing at an older executable is a conflict; inspect and
move it manually before enabling from a new installation. Moving or removing
the binary can make an existing entry unable to launch it.

Executable paths are UTF-8 XML strings, with XML metacharacters escaped once.
Spaces, non-ASCII characters and shell punctuation stay literal arguments;
there is no shell evaluation. NUL, control characters, invalid XML characters,
non-UTF-8 executable paths and generated entries beyond 16 KiB are rejected.
No executable path is silently truncated or converted lossily.

Reads admit only regular files, use nonfollowing/nonblocking open flags and
recheck the opened file and path. At most 16 KiB plus one growth-detection byte is
read. Enable requests mode 0700 for new directories and 0600 for the file,
subject to the process umask; existing permissions are unchanged. It writes and
synchronizes a private temporary file, then publishes with a same-directory
no-replace hard link. The filesystem must support that operation. A final parent
directory `fsync` confirms the mutation; failure reports that the file was already
created or removed. Abrupt termination can leave a
`.screenshots-autostart-UUID.part` file, which is not a LaunchAgent plist.

Disable rechecks file identity, size and timestamps immediately before unlink.
This detects ordinary intervening edits, but is not atomic compare-and-delete;
a hostile writer can still replace the path in that final window. Avoid editing
the same entry concurrently. Parent directory symlinks follow ordinary filesystem
semantics; a symlink at the final plist path is refused. No unrelated file is
removed as cleanup.

The implementation adds no dependencies, watchers, timers, background threads,
image allocations or network work. Its cost is bounded file metadata/content
and one temporary file only during an explicit command. Unit fixtures use
injected temporary paths and executable names, never the user's real LaunchAgents
directory. All three focused optimized fixtures passed on Linux: plist escaping
and bounds, idempotent file lifecycle, and preservation of conflicting entries.
File-level tests on Linux do not establish macOS launchd or login-time behavior.
