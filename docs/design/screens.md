# ScratchRunner screens and states (wireframes)

ASCII wireframes of every screen and state. Measurements, tokens, copy and motion are in `/DESIGN.md` (section numbers in brackets). Legend: `[ ]` button, `[x]` checked checkbox, `[ ▾]` popup, `( )`/`(•)` radio, `<>` back/forward, `~~~` wavy underline, `█` selection or caret, `░` secondary fill, `····` flexible space. Heights in points are on the left margin. Width is not to scale.

## 1. Main window, default (editor + console at bottom + navigator)  [8.1]

```
y    +------------------------------------------------------------------------------------------------+
0    | (●)(●)(●)               [Py] permute.py – Edited                                               | title band 32
32   | [Syntax: Python        ⇅][ ▶  ■  🔧]      ·····      [ <  | > ] [ ▯▯ | ▭ | ▯▯ ]                | toolbar band 40
72   +------------------------------------------------------------------------------------------------+
     | [Py] permute.py ● | [Sh] hello.sh   x | [JS] a.js   x |  +  |                              ⌄ | tab bar 32
104  +--------------------+---------------------------------------------------------------------------+
     | [▾ Documents  ⇅][<][>][⌕]|  1  ▶ def permute(a, lo, hi):                                      |
     |                    |  2      if lo == hi:                                                      |
     | > bruteforce       |  3          print(a)                                                      |
     | > Calculator       |  4      for i in range(lo, hi):   [x  Undefined name 'hi'    Fix]         |
     |   http_server.js   |  5          a[lo], a[i] = a[i], a[lo]                                      |
     | v Useful code      |                                                                           |
     |     fibonacci.m    +----------------------------------[handle 9 hit / 1 line]-----------------+
     |     permute.py  ░░ | [⌨|🌐] ·········· [Filter___] [All Output ⇅] [⌫] [⌄]                       | console toolbar 28
     |   ...              | abcdefg                                                                   |
     |                    | bacdefg                                                                   |
     |                    | Charset: _                                                                |
     | 240 (180..400)     +---------------------------------------------------------------------------+
     | [+][📁+][→📁] [Filter__] | ✓ Run Succeeded   Time 27 ms   Peak Memory 754K ····  (f) permute ⇅  Tabs: 4 ⇅  Line 9, Column 44 | 24
     +--------------------+---------------------------------------------------------------------------+
```

Regions: sidebar fill `sidebar` over vibrancy; editor `editor-bg`; console `console-bg`; bars `editor-bar`. Three 24pt bottom bars share one baseline. Hairlines: [5.2].

## 2. Main window variants

### 2.1 Console at the right, documentation open  [8.7, 8.8]

```
+--------------------+--------------------------------+---------------------------+----------------------+
| sidebar            | editor                         | console (right, 40%)      | docs 300             |
|                    |                                | [⌨|🌐] ··· [Filter][All ⇅][⌫][›] | [Search Docs___][<>][👁][⚙] |
|                    |                                | output ...                | [Python][Google][SO] |
|                    |                                |                           | ━━━ loading 2pt      |
|                    |                                |                           | (web page)           |
|                    +--------------------------------+---------------------------+                      |
|                    | status bar (spans center column)                           | [Item ⇅] 24          |
+--------------------+------------------------------------------------------------+----------------------+
```

At width < 720 the docs panel closes by itself [2.2].

### 2.2 No open document  [8.16]

```
| tab bar (empty: only  + )                                                                              |
+--------------------+---------------------------------------------------------------------------------+
| sidebar (rooted)   |                                                                                 |
|                    |                          [ 📄+ 32 ]                                              |
|                    |                     No Open Document                                            |
|                    |        Press ⌘N to create a file or ⌘O to open one.                             |
|                    |                    [ New File ]  [ Open… ]                                       |
+--------------------+---------------------------------------------------------------------------------+
```

### 2.3 Debugging (console enlarged to 60%, debug pane)  [8.9]

```
|  4 ▶|  for i in range(...)    ░░░░░░░░░░░░░░░░░░░░░░░░░░ [ ⏸ Stopped at Breakpoint ] ░░░░░░░░░░░░░░░░░ |
+---------------------------------------------------------------------------------------------------------+
| [⌨|🌐] [ ▶/⏸ ⤼ ⤓ ⤒ ] ····· [Filter][All Output ⇅][⌫][ stack | text ][⌄]                                |
| Stack            | Variables                        | Console text                                         |
| Thread: main     | v V charset = 'defg'             | Starting Debugger…                                    |
| [{}] 0 permute() |   V a = ['a','b']       2 items  | (Pdb) _                                               |
|        permute.py:9 | V lo = 0                      |                                                       |
+---------------------------------------------------------------------------------------------------------+
|| ⏸ Paused ···································································  (f) permute ⇅  Tabs: 4 ⇅  Line 9, Column 44 |
```

## 3. Toolbar band  [8.3]

```
 x=8                                                                                              x=W-8
 [ Syntax: Python           ⇅ ] 8 [ ▶ | ■ | 🔧 ] ···· flex ···· [ ‹ | › ] 8 [ ▯| | ▭ | |▯ ]
   200 (120..240), 28 high         36 36 36 =108                    28 1 28 =57    40 1 40 1 40 =122
 popup open  = pressed fill        wrench = primary tint when run settings are custom
 view toggles on = capsule-selected wash + primary icon
 hold menu (native):  Run / Run and Debug / Run Without Debugging / Compile|Run Diagnostics
```

## 4. Sidebar  [8.4]

### 4.1 File navigator

```
+------------------------------+  32  header:  [📁 Documents ⇅] [‹ ›] [⌕]
| > bruteforce                 |  24  row
| > Calculator                 |
|   [JS] http_server.js        |
| v Useful code                |  v rotates 90° (base)
|     [m] fibonacci.m          |  level indent 16
|     [Py] permute.py     ░░░░ |  selected: sidebar-accent;  current doc: medium + primary icon
|   ...                        |
+------------------------------+  24  footer:  [+] [📁+] [→📁]  [ Filter______ ]
```

### 4.2 Filter with no matches

```
|             No Filter Results             |
|        [ Search "foo"… ]                  |
|        [ Search Subdirectories ]          |
|        [ Search File Contents ]           |
```

### 4.3 Search view

```
+------------------------------+
| Search: [Browse…        ⇅] [⌕] |  32
| [File Contents + Filenames ⇅] [Options…] | 32
| [Find ⇅][Text ⇅][ ] Ignore Case |  narrow find bar (stacked)
| [ search field ___ ] [‹ ›]    |
| [ replace field __ ] [Replace|All In Selection|All]
|------------------------------|
| [Py] permute.py  ~/code   3  |  file header 28 (badge = count)
|    4  for i in range(lo,hi): |  match row (44, 2 lines)
|    9      a[lo], a[i] ...     |
| ▓▓▓▓▓░░ Searching 214 files…  [Stop] |  progress strip
| [↑][↓]        12 matches     [Done] |  footer 24
+------------------------------+
```

## 5. Tab bar  [8.5, 7.13]

```
 8
 [ [Py] permute.py ●  ]  [Sh] hello.sh   ✕   [JS] long_named_script_name…  [⟳ Py] running.py  [ + ] [⌄]
   selected: raised pill (tab-active, E1)     ●→✕ on hover        spinner replaces icon       plus / overflow
 right-click on a tab (native menu):  Run | Stop      (only those)
```

## 6. Editor details  [8.6]

### 6.1 Gutter, breakpoint, annotation, execution line

```
 ┌────┬────┬────────────────────────────────────────────────────────────────────────┬─┐
 │ ▶  │  1 │ def permute(a, lo, hi):                                                 │▒│ overview ruler 8
 │    │  2 │     if lo == hi:                                                        │ │ marks: error / warn /
 │ ●> │  3 │         print(a)           [ ⏸ Stopped at Breakpoint ]                   │░│ notice / find / bp
 │    │  4 │     for i in range(lo, hi):   [ ⓧ Undefined name 'hi'   Fix ]           │▒│
 │    │  5 │         a[lo], a[i] = a[i]~~~~~~                                        │ │
 └────┴────┴────────────────────────────────────────────────────────────────────────┴─┘
   16   n    8 pad                                                                       
 ●  = breakpoint tag        ▶ = execution arrow (debug-arrow) + debug-line tint on the whole row
 [ pill ] = annotation overlay, right aligned, tinted diag-*-bg
```

### 6.2 Completion panel  [8.6.3]

```
     for i in ran█
          +-----------------------------------------------+
          | [f] range(stop)                       range   |   <- selected: menu-highlight
          | [f] range(start, stop[, step])        range   |
          | [C] RangeError                        class   |
          | [V] rangeValue                        var     |
          +-----------------------------------------------+  E2, radius-lg, max 8 rows of 24
                                                   +----------------------------+
                                                   | range(stop)                |  doc popover
                                                   | Return a sequence of ints… |
                                                   +----------------------------+
```

### 6.3 Find bar, full width  [8.6.4]

```
+----------------------------------------------------------------------------------------------------+
| [Find ⇅] [ ⌕ Find                       3 of 12 ✕ ] [Text ⇅] [x] Ignore Case  [˄ ˅]   [ Done ]      |  36
| [ Replace ______________________________ ]     [ Replace | All In Selection | All ]               |  32
+----------------------------------------------------------------------------------------------------+
```

States: `12 matches`, `No matches`, invalid regex: field outline in danger + `Regex Invalid`.

### 6.4 Status bar items  [8.6.5]

```
| ⟳ Compiling…                                                    (f) main ⇅   Spaces: 4 ⇅   Line 3, Column 9 |
| ✕ Compile Error  Exit Status 1  Time 412 ms                      (f) main ⇅   Spaces: 4 ⇅   2 Errors, 1 Warning |
| ✓ Run Succeeded  Time 27 ms  Peak Memory 754K                    ...                       3 Selections, Identical, 3 Lines, 0 Characters |
| ⚠ Rust · rust-analyzer failed: the server closed its output  [Show Error]    ...                         |
```

## 7. Console states  [8.7]

```
[⌨|🌐] [debugger group]·········· [👁] [⌕ Filter____][All Output ⇅][⌫][ ⌦ | ⌨ ][ ⧉ | ▭ | ▯ ][⌄]
 Terminal  Web    (debugging)       Live                                inspector (web mode)   hide
```

- Idle (opened manually): centered caption `Output appears here when you run the document (⌘R)`.
- Large output: native alert `Large output to the console detected. Would you like to stop the run?` [Stop] [Continue Run].
- Scrolled up while output arrives: a `⌄` capsule button at the bottom right (`Scroll to bottom`).
- Web mode: `[‹ ›] [Website ⇅] [URL ______________] [Inject Into: ⇅]` above a web view; Live Updating on.

## 8. Documentation panel states  [8.8]

```
 normal                                 no sources                    offline
+--------------------------+            +----------------------+      +----------------------+
|[Search Docs____][‹›][👁][⚙]|            | (header)             |      | (header)             |
|[Python][Google][SO][Find]|            |       [📖 24]        |      |       [wifi-off]     |
|━━━                       |            | No Documentation     |      | No Internet Connection|
| (web page)               |            | Sources              |      | An internet connection|
|                          |            |   [ Configure… ]     |      | is required to search |
| [Item ⇅]                 |            |                      |      | …   [ Try Again ]    |
+--------------------------+            +----------------------+      +----------------------+
```

## 9. Run Settings sheet  [8.10]

```
            +----------------------------------------------------------+
            | Run Settings                                             |  subhead + caption (doc name)
            | permute.py                                               |
            | Run Command:                                        (?)  |
            | +------------------------------------------------------+ |
            | |1 python3 $filename                                   | |  CodeField 2 rows
            | +------------------------------------------------------+ |
            | [ Run command is specific to this file.  [Revert][Make Default] ]   banner (when changed)
            | Arguments:                                               |
            | [ ______________________________________________ ]       |
            | Compile Flags:  (only with a compile script)             |
            | [ ______________________________________________ ]       |
            | Program Input:                                           |
            | [                                                  ]     |  TextArea 80
            |                                          [ Close ]       |  primary, lg 32
            +----------------------------------------------------------+   500 wide, radius-xl, E3
```

## 10. QuickNav  [8.12]

```
 empty query                         results                                        no results
+--------------------------+   +-----------------------------------+      +--------------------------+
| ⌕ Open Quickly       (⟳) |   | ⌕ perm                        (⟳) |      | ⌕ zzzz                   |
+--------------------------+   |-----------------------------------|      |--------------------------|
        56                     | [Py] permute.py          ░░░░░░░░ |      |        No Results        |
                               |      ~/code/algos                 |      +--------------------------+
                               | [Py] test_permute.py              |
                               | [(f)] permute                      |
                               | [⇧] Set Language: Perl            |
                               +-----------------------------------+
```

480 wide, centered, top at 22% of the window height.

## 11. Prompt sheet and Go to Line  [8.11]

```
+------------------------------------------+      +------------------------------+
| Go to Folder                             |      | Go to Line                   |
| Go to the file or folder:                |      | Enter a line number, or      |
| Open Path:                               |      | line:column.                 |
| [ ~/Documents/proj█                    ] |      | Line Number                  |
|   ghost completion: ~/Documents/projects |      | [ 12█                      ] |
|                    [ Cancel ]  [  Go  ]  |      |        [ Cancel ]  [  Go  ]  |
+------------------------------------------+      +------------------------------+
 invalid input: sheet shakes, field outline = danger, reason caption below the field
```

## 12. Preferences  [8.13]

### 12.1 Window shell

```
+------------------------------------------------------------------------------------------+
| (●)(●)(●)                      Preferences – Languages                                    | 32
+------------------------------------------------------------------------------------------+
|   [⚙ ]        [ ✎ ]        [{}]        [🎨]        [⌘]        [>_]        [⚙⚙]            | 72
|  General   Text Editing  Languages    Themes   Key Bindings   Shell     Advanced          |
|              (selected: muted fill, radius-lg)                                              |
+------------------------------------------------------------------------------------------+
| content area (background), padded 20                                                       |
```

### 12.2 General (stacked sections, FormRow: label column 200)

```
 Documents
                    [x] Enable Auto Save and Versions
                    [x] Show all filename extensions
                    [ ] Ask to save untitled documents when quitting
                    [x] Use templates for new files
                    [ ] Allow filename suggestions from macOS
 Default File Encoding:  [Unicode (UTF-8)       ⇅]     [ Edit File Type Associations… ]
 ----------------------------------------------------------------------------------------
 Tabs
                    [x] Prefer opening new documents in tabs
                    [ ] Hide tab bar by default
 Insert New Tabs:   [After existing tabs        ⇅]
 ----------------------------------------------------------------------------------------
 Console            Console Position: [Bottom ⇅]    [x] Hide console automatically
 File Navigator     [ ] Show hidden files and folders     [ ] Sort folders on top
 Notifications      [x] Show notifications while the app is inactive
```

### 12.3 Text Editing (segmented categories)

```
                  [ Appearance | Indentation | Behavior | Completion | Diagnostics ]
 Font:            Fira Code 13pt  [ Choose Font… ]
                      [ ] Ligatures   [x] Antialiasing   [x] Smoothing
                  [x] Syntax highlighting
 Line Spacing:    [ 1.20 ⇵ ]
                  [x] Show navigational markers in scroller
                        [x] Keep markers visible when scroller is hidden     <- child, indent 20
                  [x] Wrap lines   [x] Allow scrolling past end of document
                  [ ] Show page guide at column [ 80 ⇵ ]
                  [x] Show invisible characters: [\t \n\r  ] [In selected text + nonconforming indentation ⇅]
 Insertion Point: [Bar ⇅]  [x] Blink  [x] Animate  [x] Visible with selection
 ...
```

### 12.4 Languages

```
+ 220 -----------------+  +-----------------------------------------------------------------------+
| AppleScript      [x] |  |            [ Settings | Templates | Docs | Diagnostics ]                  |
| C                [x] |  | +-------------------------------------------------------------------+     |
| C# (.NET)        [x] |  | | [x] Language uses compile script (?)           [ Edit Script ]     |     |
| Go               [x]░|  | | Compile Flags: [____________________________]                    |     |
| ...                  |  | | ------------------------------------------------------------------ |     |
+----------------------+  | | Run Command: (?)                       I/O Encoding: [Auto (UTF-8) ⇅]|     |
[+][−][⋯]                 | | +--------------------------------------------------------------+   |     |
                          | | |1 go run $filename                                            |   |     |
                          | | +--------------------------------------------------------------+   |     |
                          | |   $filename  The filename of the source file                       |     |
                          | | File Extensions: [go           ]   Syntax Mode: [Go ⇅]             |     |
                          | | Console Type: [Terminal ⇅]  Debugger: [lldb ⇅]  Indentation: [Default ⇅]|     |
                          | +-------------------------------------------------------------------+     |
```

Templates tab:

```
| +--------------------------------------------------------------+ |
| | Name        | Default | Extension | Name on Creation           | |
| | Main File   | Default | go        | [ ]                        | |
| +--------------------------------------------------------------+ |
| [+][−] [ Unset as Default ]                                  (?) |
| Template:                   Use <##> to represent text insertion point. |
| +--------------------------------------------------------------+ |
| |1 package main                                                | |
| |3 func main() {                                               | |
| |4     <##>                                                    | |
| +--------------------------------------------------------------+ |
```

Docs tab: intro caption, table `[x] Name | Type ⇅ | URL | POST | CSS Edit | JS Edit`, `[+][−]`, `Global entries (available for all languages):` table, checkbox `When invoked from the editor, always use:` + popup.

Diagnostics tab:

```
| [x] Automatically add built-in diagnostics                          |
| v [ My Diagnostic          ]                     [ Manually ⇅ ]     |
|     Generate diagnostics using: [Compile Script ⇅]                  |
|     [ ] Use custom compile flags: [ ______ ] [ ] Add to existing flags |
|     Mode to interpret output: [Regular Expression ⇅]                |
|     +----------------------------------------------------------+    |
|     | ^(?<file>[^\s].*?):(?<line>\d+):(?<column>\d+):? (?<message>.*)$ |
|     +----------------------------------------------------------+    |
|     Possible capture groups: file, line, column, message, …         |
|     [ Edit Script ]   Keyboard shortcut: [ Click to set … ]  [ Perform Test Run… ] |
| > [ Invalid Syntax ]                                 [ On Edit ⇅ ]  |
| [ Options… ]  [ Auto-Fixes… ]  [ Ignored… ]                        |
```

### 12.5 Themes

```
+ Color Themes 180 -+  Foreground Background Selection CurrentLine Cursor Invisibles
| [▪] ScratchRunner ☾|   [▬▬]      [▬▬]       [▬▬]      [▬▬]       [▬▬]    [▬▬]
| [▪] ScratchRunner ☼|  +--------------------------------------------------------------+
| [▪] Monokai       ☾|  | Element                    | FG | BG | B | I | U |             |
| ...                |  | Comments  (drawn in its own colors)  [■] [ ] [ ] [x] [ ]       |
+--------------------+  | Keywords                              [■] [ ] [x] [ ] [ ]       |
[+▾][−][⋯]              +--------------------------------------------------------------+
                        [+][−]  Scope selector: [ comment, punctuation.definition.comment ]  [x] Show all
                        Use ⌃⌘P in the editor to show the active scope
                        (•) Use the chosen theme for all appearances
                        ( ) Use separate themes for different appearances:  Light: [▾]  Dark: [▾]
                        [x] Match the app appearance to the theme
```

### 12.6 Key Bindings

```
 Key Bindings Set: [Default ⇅]  [Duplicate] [Rename] [Delete]
          [ Menu Items | Commands | Compound Commands | Global Compound Commands ]
 +-----------------------------------------------+------------------------------------------+
 | [x] File ‣ New Tab        [Reset] [ ⌘T      ]  | { JSON compound-command editor }          |
 | [x] Run ‣ Stop            [Reset] [ ⌘.      ]  |   or: Select a compound command to      |
 | [x] Find ‣ Find Mode ‣ …  [Reset] [⌥⌘E, R   ]  |   edit its JSON.                         |
 +-----------------------------------------------+------------------------------------------+
 [ Filter ______ ] [+] [−] (?)                                          Invalid JSON
```

### 12.7 Shell and Advanced

```
 (•) Automatically determine PATH environment variable                       [⟳]
 ( ) Use custom PATH:
 +--------------------------------------------------------------------------------+
 | /usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin                                    |
 +--------------------------------------------------------------------------------+
 [ ] Invoke shell in login mode when running code
     When invoked in login mode, the shell executes startup scripts such as ~/.profile.
 Shell: [ /bin/bash ]    Using a shell other than bash is not recommended and may cause undesirable behavior.
 Shell Variables:
 +-----------------------------+-----------------------------------------+
 | Variable                    | Value                                   |
 | TERM                        | dumb                                    |
 +-----------------------------+-----------------------------------------+
 [+][−]  [ Reset ]
```

Advanced: single FormRow column, sections `Clang completion and diagnostics`, `Debugging` [8.13.1].

## 13. Alerts, About, Help  [8.14, 8.15]

```
 native NSAlert (sheet on the window)       About (320)                   Help (760 x 560)
+-------------------------------------+   +-------------------+          +-----------+----------------------------+
| (app icon)  "permute.py" is         |   |       [ ▶ ]       |          | Topics    | [Search Help______]  < >   |
| currently running. Are you sure you |   |   (80 tile)       |          | Key Bind… | Run Command                 |
| want to close it?                   |   |  ScratchRunner    |          | Run Comm… | ...                         |
| Closing the document will stop the  |   |  Version 0.1.0 (1)|          | Environ…  |                             |
| run.                                |   |  Run code in any  |          | Compile…  |                             |
|                  [Cancel]  [Close]  |   |  language, …      |          | Scope Se… |                             |
+-------------------------------------+   |  Software Attrib… |          | Templates |                             |
                                           |  Visit ScratchR…  |          | …         |                             |
                                           +-------------------+          +-----------+----------------------------+
```

## 14. Banners, toasts and error states  [7.23, 8.16]

```
 Banner (inline, editor top)
 | ⚠ "permute.py" was changed on disk while you have unsaved changes.   [Reload from Disk] [Keep My Version]  ✕ |

 Toast (bottom center, 16 above the status bar)
                 +-------------------------------+
                 | ✓ Reloaded from disk          |
                 +-------------------------------+

 Sidebar unreadable folder
 | ✕ Couldn't read this folder.   [Try Again] |
```
