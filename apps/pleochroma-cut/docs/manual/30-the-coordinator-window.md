# 30. The Coordinator Window

## What You Will Do

Since 0.12.0 the coordinator, the program that renders for your phones and other
computers and keeps your design library available to them, has a window of its own.
This chapter shows how to start it, what its pages hold, how to pair a phone (by QR code
or by link) and another computer, how it runs in the tray, how to monitor or manage a
coordinator on another computer, and how the old command line still works.

The coordinator is the program `pleochroma-worker`. Chapter 10 explains what a
coordinator is, how this app connects to one and how to run one as a Linux service. This
chapter is about the window. Its command reference, with every flag, is the worker's own
README (`apps/pleochroma-worker/README.md`), and the trust model behind pairing is in
`apps/pleochroma-worker/docs/security.md`.

## Starting the Coordinator

Start `pleochroma-worker` with no arguments, or double-click it, and the window opens. Its title is
**Pleochroma Coordinator**. Two options change how it starts:

```
pleochroma-worker
pleochroma-worker --minimized      # start hidden in the tray or the menu bar
pleochroma-worker --config <file>  # use another settings file
```

The window needs a build with the window (the default build has it). On a computer with
no display the program says so and tells you to use `--headless` (see "Running Without a
Window" below).

Only one coordinator window runs for each user and data folder. Starting a second one
brings the first to the front and exits.

The window is at least 640 by 480 pixels. Below a certain width the sidebar folds to its
icons.

## The First-Run Wizard

The first time, a wizard asks what this computer is for. A line of numbered stops along the
top shows where you are, and **Back** and **Next** move between them.

1. **Start.** "What would you like to do?" Choose **Run a coordinator on this computer**
   ("Phones and render computers connect here.") or **Monitor or manage a coordinator on
   another computer** ("No server runs here. You connect with a code."). The second choice
   skips to the last step. You can add or switch coordinators later from the name at the
   top of the sidebar.
2. **Name.** "Name this coordinator." Phones and other computers show this name. It starts
   as the name of this computer and you can change it later in Settings.
3. **Library.** "Where are your designs?" Your phones browse designs from one library
   file on this computer. Choose **Choose a library file** (the `facet_diagrams.sqlite` file of
   Pleochroma), **Rendering only (no library)** (phones can render here but cannot browse
   designs), or **Use an existing coordinator folder** (it takes over a setup that ran
   headless and keeps every paired device).
4. **Network.** "Who can connect?" Choose **Devices on my network** or **Only this
   computer** (phones and other computers cannot connect). Your system may ask to allow
   Pleochroma on the network: choose Allow for private networks.
5. **Rendering.** "Render on this computer." Choose **Graphics card and processor**,
   **Graphics card only** (it keeps the processor free for other work) or **Processor
   only** (slower, but it works on every computer).
6. **Done.** "Ready to start", with a summary of Name, Library, Network and Rendering and two last choices:
   **Start with Windows (in the tray)** (on Linux and macOS it reads **Start at login (in
   the tray)**; it runs only while you are logged in) and **Keep this computer awake while
   devices are connected or jobs run** (it can still sleep when nothing is going on). The
   button reads **Start the coordinator**, or **Add a coordinator** if this computer only
   monitors.

When the wizard finishes it creates the certificate authority and the server certificate,
starts the coordinator and opens the pairing dialog, so the first device can connect at
once.

## The Pages

The sidebar lists the pages. Its top shows the coordinator you are looking at (see "Managing
Another Coordinator" below). A badge on **Devices** and **Jobs** shows a count, and the bar at the top of the window
shows the status ("Running"). A message banner under the top bar says what just happened and has a
**Dismiss message** button.

- **Overview.** The address and the uptime, three counters (**Devices online**, **Render
  computers**, **Jobs**), what is rendering right now ("Nothing is rendering." when idle), and
  **RECENT ACTIVITY** since the window opened. Two buttons start pairing: **Connect a phone**
  and **Connect a computer**.
- **Devices.** The phones and computers paired to this coordinator, with their status and
  connection. **Connect a device** pairs another. The menu on a row (**More**) holds **Rename**,
  **Copy fingerprint** and **Remove access**; removing access disconnects the device at once
  and it can pair again with a new code. If **Accept any client certificate** is on in
  Settings, Remove access has no effect, and the page says so in a warning.
- **Render computers.** Other computers that lend their graphics card and processor.
  **THIS COMPUTER** has a switch **Render on this computer** and a **CPU threads** stepper. **CONNECTED
  RENDER COMPUTERS** lists the others, with a **Disconnect** button each, and **ALLOWED
  RENDER COMPUTERS** the ones that may join; **TOTAL CAPACITY** adds them up. **Add a render
  computer** pairs one (see "Pairing Another Computer").
- **Jobs.** The renders the coordinator keeps, under **RENDERING NOW** and **SAVED JOBS**, with a filter (**All**,
  **Running**, **Queued**, **Done**, **Failed**) and the columns owner, label, size, samples, state, progress,
  created and route. **Cancel job** stops a running job. **Delete job** asks first ("The job and its picture
  are removed from this coordinator. This cannot be undone."). **Open jobs folder** shows the files.
- **Library.** The design library your devices can search and open: its file, the number of
  designs and the note "Read only. The coordinator never changes your library." **Change...**
  picks another file and **Open folder** shows it. A change that needs a restart shows **Restart now**.
  See "A Coordinator With No Library" below.
- **Help another coordinator.** The other way round: this computer lends its render power to a
  coordinator elsewhere (see "Lending This Computer to Another Coordinator"). Only on this computer's
  own coordinator.
- **Tools.** Two small tools, only on this computer: **Render a scene file** and **Run without the window**
  (see "Tools").
- **Log.** The coordinator's log. The choice **Problems** (errors and warnings only), **Info**
  (normal operation and problems) or **Details** (everything, for diagnosing) sets how much shows. **Search the log** filters the lines,
  **Copy visible lines** copies them and **Open log folder** shows the files. Actions from a remote
  administrator are recorded here: who, what and when.
- **Settings.** Sections **General**, **Network**, **Security**, **Performance**, **Storage** and
  **Advanced** (see "Settings").

## Pairing a Phone

Choose **Connect a phone** (on Overview, in the sidebar, or in the tray menu). The dialog makes a
one-time code that works for three minutes; a countdown shows the time left. It shows:

- the code as a **QR code** and, under **Pairing link**, as a link with the secret part
  hidden on screen;
- **HOW TO PAIR**: "On your phone, open More → Remote", tap the scan button and "Point the
  camera at this code, then tap Pair";
- **IN THIS CODE**: the address and the certificate the phone will trust, and the access it gets.

The phone must be on the same Wi-Fi as this computer. "Closing this window cancels the
code." If the code runs out the dialog says **Code expired** and **Show a new code** makes
another; **Code paused** and **Can't make a pairing code** say what is wrong.

When the phone has paired, the dialog says "Paired from" and the phone's address, and asks you
to **Name this device** ("Shown in Devices. You can rename it later.") and **Confirm**. Then
**Done** closes it and **Connect another** shows a new code. If your phone says the code was
already used, someone else paired first: choose **Not my device — remove access**.

### On the phone

On the phone open **More**, then **Remote**. The section **PAIR WITH A COMPUTER** reads "On the
computer, choose Connect a phone, then scan its code or paste its pairing link here. A link
from another app always asks you first."

- **Scan QR code** opens the camera. Point it at the code in the coordinator window.
- **Paste pairing link** shows a **Pairing link** field (its text is hidden as you type) and a
  **Continue** button. Use this when you copied the link from the coordinator on a
  computer.

Neither connects at once. A sheet comes up from the bottom, **Pair with *name*?**, with the
coordinator's **Address**. Choose **Pair** to connect or **Cancel** to forget the link.
When this phone is already paired with another coordinator, the sheet adds "This replaces
*name*" and "This phone forgets that pairing.", and the button reads **Replace and pair**. The
sheet says **Cannot pair** with the reason when the link has expired or cannot be used.

A phone paired this way finds the coordinator again if its address changes, because the
coordinator announces itself on the network (Settings → Network → Discovery). The older way
still works: **Enter details by hand** under the pairing buttons, with a one-time token.

## Pairing Another Computer

Choose **Connect a computer**. The dialog is the same, but it shows the link instead of a code
to scan:

1. Under **Pairing link** copy the link (the secret part is hidden on screen, and the copy stays out of the
   clipboard history; the clipboard is cleared when the code expires).
2. **ON THE OTHER COMPUTER**: open Pleochroma on the other computer, choose **Remote
   Coordinator → Paste pairing link** and paste. In the Pleochroma app that is **File >
   Remote Rendering...** and the section **Connect with a Pairing Link** (Chapter 10).

A computer that is to **lend its render power** instead uses **Add a render computer** on the
**Render computers** page: "Another computer lends its graphics card and processor to this
coordinator." The steps shown are: open Pleochroma and choose **Help another coordinator**,
paste the pairing link, choose **Connect**. Afterwards the dialog says "A render computer
connected from" the address, and "It shows under Allowed render computers and joins when it starts.
Rename or remove it in Devices." **Add another** shows a new code.

## Lending This Computer to Another Coordinator

The page **Help another coordinator** ("Lend this computer to another coordinator") turns this
computer into a render computer for a coordinator elsewhere: "It sends render work here. Your own
library and devices are not shared." Paste the link from that coordinator into **Pairing
link**; when it is valid the page shows the **Coordinator**'s name ("link valid") and a
**Start** button. **Stop** ends the help. The page also has **Slots** (tiles rendered at the same time;
one slot per 8 threads is a good start), **CPU threads** (Automatic uses every thread), **Use** (what
renders) and **Certificate folder** with **Choose folder...** and **Use the default**. A
**SAVED CONNECTION** is kept, so the next start reconnects.

## A Coordinator With No Library

A coordinator started with **Rendering only (no library)** has no design library. The **Library** page says
"No design library", with the badge **Rendering only**: "Devices can send renders here, but they cannot search or open designs on this computer. Choose a
library to change that." **Choose a library...** adds one later. Phones can still render,
and the Pleochroma app can send renders to it, but cannot browse its designs.

## Tools

- **Render a scene file** renders a scene file on this computer, without a phone: pick the
  **Scene file**, the **Size** in pixels, the **Samples** per pixel, the **Engine**
  (**Path-Traced** or **Ray-Traced**), the **Device** (**GPU** or **CPU**) and the **Output
  file**, and start it. When it finishes the page says "Saved" with the file and offers **Open the
  picture** and **Show in folder**.
- **Run without the window** shows the command that starts the coordinator in the background
  with the same settings (it holds file paths and no secrets) and a **Copy** button. Choices
  with no command-line option, such as where jobs are stored, stay with the window.

## Settings

Settings are saved in `coordinator.toml` (Windows: `%APPDATA%\Pleochroma`; Linux:
`~/.config/pleochroma/`; macOS: `~/Library/Application Support/Pleochroma/`). A change
that needs a restart shows "Restart the coordinator to apply the changes. Devices are
disconnected for a moment." The **Restart** card of the settings page, or **Restart now**, does it.

- **General.** The coordinator's **Name**; **Closing the window keeps the coordinator running in the
  tray**; **Keep this computer awake while devices are connected or jobs run**; **Start at login (in
  the tray)**; **Check for a newer version in the background**; and **Appearance** with **High contrast**.
- **Network.** **Who can connect** (the **Address** to listen on and the **Port**; three more ports
  follow it), **Limits** (**Connections** and **Per address**), **Can phones reach this computer?**
  (the network profile, the firewall and the last contact from a device: a test from this computer to itself cannot
  pass through the firewall, so only a real device proves the connection), **Pairing** (**Allow pairing new
  devices**; off closes the pairing ports and paired devices keep working), **Discovery**
  (**Announce this coordinator on the network**; paired devices find it again after an address change) and
  **Extra names for the certificate** (a VPN address or a DNS name, separated by commas).
- **Security.** **Remote administration** (below), **Backup and restore** and **Accept any client certificate**.
- **Performance.** **Jobs** (memory per job, jobs per device and the small-picture rule),
  **Render computers** (**Accept render computers**, **Live preview uses** this computer only or all,
  **Preferred computer**, **Network traffic**) and **Processor** (**CPU threads**).
- **Storage.** **Kept results** (**Keep finished jobs**, **Days to keep finished jobs**, the cache sizes) and
  **Data folder** (certificates, jobs, cache and logs; **Open data folder**, **Use an existing coordinator folder...** and **Back to the default folder**).
- **Advanced.** **Port overrides** for viewers, viewer pairing, render computers and render computer
  pairing, and the red setting **Accept any client certificate**.

**Backup and restore** writes one file with the keys, the paired devices and the settings, sealed with a
password you choose (**Back up...**, **Restore...**). Keep it safe: it can pair devices.

**Accept any client certificate** skips the list of paired devices. Anything with a certificate from this
coordinator's authority can connect, and **Remove access** stops working. Turn it on only to recover from
a problem, and turn it off again afterwards; the switch asks "Accept any client certificate?" first.

## The Tray and Starting at Login

Closing the window does not stop the coordinator if **Closing the window keeps the coordinator running
in the tray** is on: it keeps running in the Windows or Linux tray, or the macOS menu bar. Where a Linux
desktop has no tray, closing minimizes the window instead. The tray menu has **Open**, the status, **Connect a phone**
and **Quit**. **Quit** in the window's **More** menu stops the coordinator; **Open data folder** is in the same menu.

**Start with Windows (in the tray)** (**Start at login (in the tray)** on Linux and macOS) in the wizard and in Settings
registers the coordinator to start when you log in. A start at login opens **in the tray only**: the program starts with `--minimized`,
so no window appears. Open it from the tray when you need it. The coordinator runs only while
you are logged in. A coordinator that must run all the time without anyone logged in is a service
(Chapter 10, "Running the Coordinator or a Render Computer as a Linux Service").

## Managing Another Coordinator

The same window monitors or manages other coordinators, headless or with a window. The
**coordinator switcher** at the top of the sidebar lists **COORDINATORS**: **This computer**, every
coordinator you added, and **Add a coordinator...**. Choose one and every page shows that
coordinator; a banner and a different icon say that you are looking at a remote one. The top bar's
**Disconnect** goes back to this computer and keeps the pairing saved. The menu of a coordinator in the
list has **Disconnect**, which forgets it ("This computer forgets the pairing with *name*."; the
button reads **Disconnect and forget**). **Help another coordinator** and **Tools** exist only for this
computer.

### Allowing remote administration

It is **off by default**. To allow it, open **Settings → Security → Remote administration** and switch on
**Allow other computers to monitor or manage this coordinator**. Restart the coordinator
when the page asks ("Restart the coordinator to start accepting remote administration."). Codes for
remote administration are made here only, never remotely. A headless coordinator takes the flag
`--allow-remote-admin` on `serve`.

Then **Create an access code**: pick the **Access level**, enter the **Name** of the computer
that will use it (for example "Laptop 2") and choose **Create code**. The code works once and
expires after 3 minutes. It shows as a link, a QR code and a code. The two levels are:

- **Monitor only** sees everything and changes nothing. Its actions that would change something are
  greyed out with the reason "You have monitor access."
- **Full administration** can pair devices, remove access, cancel jobs and change settings on this coordinator.
  Give it only to a computer you trust.

**People and computers with access** lists who has a level, and **Remove** takes it away at
once. **Activity** says "Remote actions are recorded in the Log (who, what, when)." with an **Open Log** button.

### What a remote administrator can never do

Some actions can only be done in the window of the coordinator itself, whatever the level:
network, port and address changes, the remote-administration switch, resetting the certificate
authority, rotating secrets, exporting a backup, stopping the coordinator and making new Monitor or
Admin codes. A remote administrator asking for one is refused in plain words, and
nothing changes. Settings of the other kinds, and restarting, are open to **Full administration**.

### Adding a coordinator

Choose **Add a coordinator...** in the switcher. There are two ways:

- **Pairing link.** Paste the link into **Pairing link** (**Paste** takes it from the clipboard). The dialog
  shows **THIS LINK CONTAINS** the coordinator's name, the **ACCESS** level and when it **EXPIRES**
  ("Access: Monitor only" or "Full administration"), then **Connect**.
- **Code and address.** Enter the **Address (host:port)** (for example `192.168.178.40:7878`) and the **Code**. The code decides the
  access: monitor only or full administration, set when the code is made.

"Where do I get a code?" On a coordinator with a window: **Settings → Security → Remote administration →
Create code**. On a server without a window, run `pleochroma-worker --headless cert issue-token --role admin --name <name>`
(or `--role monitor`); it prints a code, a link and a QR code in the terminal. Add `--address` to name the address
that goes into the link and `--no-qr` to leave the QR code out. After a first connection the window reconnects by itself,
and finds the coordinator again if its address changes.

## Running Without a Window

`--headless` runs the program as before, without ever opening a window: `pleochroma-worker --headless serve ...`,
`--headless render ...`, `--headless join ...`, `--headless cert ...`. Use it in every service and script.

- **Deprecation.** A command given without `--headless` still runs in 0.12.1, but prints one warning on
  standard error: `warning: "serve" without --headless is deprecated; add --headless`. Change scripts, service
  files and shortcuts now. A line with no command, or only `--minimized`, `--config <file>` or the other global flags, opens
  the window.
- **The Windows pair.** The Windows program with a window has no console, so a terminal would get its prompt back
  at once and lose the output and the exit code. `pleochroma-worker.com` sits next to `pleochroma-worker.exe`: `cmd`
  and PowerShell start a `.com` before an `.exe`, so typing `pleochroma-worker --headless serve ...` in a
  terminal runs through it, with the output, Ctrl-C and the exit code of the real program. Keep both files in
  the same folder. A build without the window keeps its console and needs no twin.

## Notes for the Android App

- **Back.** The Back button (or gesture) steps out of whatever is open, one layer at a time: a sheet,
  a dialog, a page of More. When nothing is open it goes to the Library, and on the Library the first Back says
  "Press Back again to exit"; a second Back within two seconds closes the app.
- **Local renders.** A Path-Traced render on the phone itself has three quality choices, **16 samples**,
  **32 samples** (the default) and **64 samples**. 64 must be picked; it takes longer and heats the phone more.
  A Remote render uses the coordinator instead and is not limited by these.

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| The window does not open and the program talks about `--headless` | The computer has no display. Run it with `--headless`. |
| A second start shows the window that is already running | One coordinator window runs for each user and data folder. |
| A phone cannot scan or connect | It must be on the same Wi-Fi. Check Settings → Network → "Can phones reach this computer?", and choose Allow when the system asks about the private network. |
| "Code expired" | A code lives three minutes. Choose **Show a new code**. |
| A removed device still connects | **Accept any client certificate** is on. Turn it off. |
| A paired phone cannot find the coordinator after an address change | Check that **Announce this coordinator on the network** (Settings → Network → Discovery) is on. |
| After a restart the coordinator is not running | A start at login opens only in the tray; look for its icon. A coordinator that must always run is a service (Chapter 10). |
| An older app or worker is refused | Every computer must run the same release (protocol 45): update the apps, the coordinator and all workers together. |

## Next Steps

Chapter 10 explains how this app connects to a coordinator, transfers and library mirroring.
Chapter 23 collects renders as jobs, which run on a coordinator too.
