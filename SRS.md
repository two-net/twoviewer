# TwoViewer — Software Requirements Specification

| | |
|---|---|
| **Product** | TwoViewer |
| **Document** | Software Requirements Specification (SRS) |
| **Version** | 0.1 (draft for review) |
| **Date** | 2026-10-24 |

> This SRS describes **what** TwoViewer must do, not how it is built. It deliberately prescribes no technology stack: no programming languages, frameworks, libraries, or hosting products. The only technology constraints are the ones set by the stakeholder: **PostgreSQL** with a **code-first ORM**, **automatic migrations**, and **no partitioning** (CON-08, §5.1).

## Contents

- [1. Introduction](#1-introduction)
- [2. Overall description](#2-overall-description)
- [3. Functional requirements](#3-functional-requirements)
- [4. User interface requirements](#4-user-interface-requirements)
- [5. Data requirements](#5-data-requirements)
- [6. External interface requirements](#6-external-interface-requirements)
- [7. Non-functional requirements](#7-non-functional-requirements)
- [8. Open issues](#8-open-issues)
- [Appendix A. Traceability](#appendix-a-traceability)

---

## 1. Introduction

### 1.1 Purpose

This document specifies the requirements for **TwoViewer**, a remote access and remote support service comparable in purpose to TeamViewer. It is the reference for the design, implementation, testing, and acceptance of the first release.

### 1.2 Product scope

TwoViewer lets people:

- **reach their own computers** from any web browser (unattended access);
- **help someone else** by viewing and controlling that person's computer with their explicit consent (attended support);
- **link two of their own computers** through an end-to-end encrypted, point-to-point VPN.

TwoViewer is a free, hosted service (SaaS) for individual user accounts. There are no paid plans and no organizations or tenants.

| ID | Goal |
|---|---|
| G1 | Browser-only for the person in control: all usage happens in a web browser. |
| G2 | Safe by default: explicit consent, visible session indicators, end-to-end encryption, and a complete audit trail. |
| G3 | Simple to build and operate: one server application, one PostgreSQL database, one agent. |
| G4 | Pleasant to use: an animated, theme-aware interface and an optional Vim mode for keyboard-driven remote control. |

### 1.3 Definitions and acronyms

| Term | Definition |
|---|---|
| Attended session | A session to a device where a person at that device approves the connection with a session code and a consent prompt. |
| Audit event | An immutable record of a security- or session-relevant action: who did what, when, from where, and with what result. |
| Code-first | An approach in which the data model is defined in application code, and the database schema and its migrations are derived from that model. |
| Controller | The signed-in user who views or controls a remote device from a browser. |
| Device | A computer running the Host Agent. |
| Host Agent | The TwoViewer component that runs on a controlled computer. It captures the screen, injects keyboard and mouse input, exchanges clipboard text and files, and terminates VPN connections. Apart from its installer, its only user interface is the session indicator. |
| Host page | The browser page the person at a device uses to get a session code, answer consent prompts, and end sessions. |
| Migration | A versioned, ordered change to the database schema. |
| Operator | A member of the team that runs the TwoViewer service. |
| Pairing | Linking an installed Host Agent to a user account. |
| QuickShare | The run-once mode of the Host Agent for attended support; it needs no installation. |
| Relay | The server function that forwards encrypted session traffic when the two endpoints cannot connect directly. It cannot decrypt that traffic. |
| Remote participant | The person at the device in an attended session. An account is not required. |
| Resolved theme | The theme actually displayed (Light or Dark). With the System setting, it follows the operating system's or browser's color-scheme preference. |
| Session | A time-bounded connection between a controller and a device: remote control or VPN. |
| Session code | A short-lived, single-use, 9-digit code that identifies a device for one attended session. |
| Session indicator | The always-on-top notice the Host Agent shows on a controlled device during every session (IF-01). |
| Unattended access | Connecting to one of your own devices with nobody present to approve. |
| Vim mode | An optional, modal keyboard scheme for controlling remote sessions, modeled on the Vim text editor (§3.6). |
| VPN connection | A point-to-point, end-to-end encrypted virtual network link between two devices of the same user (§3.7). |

| Acronym | Meaning |
|---|---|
| E2E | End-to-end (encryption) |
| MFA | Multi-factor authentication |
| ORM | Object-relational mapping |
| RPO / RTO | Recovery point objective / recovery time objective |
| SaaS | Software as a service |
| TLS | Transport Layer Security |
| UTC | Coordinated Universal Time |
| VPN | Virtual private network |
| WCAG | Web Content Accessibility Guidelines |

### 1.4 Conventions

- Requirements use **shall** and have a unique, stable ID of the form `AREA-NN`. Each has a priority:
  - **Must**: required for the first release.
  - **Should**: expected in the first release; deferred only with stakeholder agreement.
  - **Could**: desirable; the first candidate for deferral.
- Constraints (`CON-NN`) are always mandatory.
- Numbers such as limits, timeouts, and retention periods are defaults that operators can change without code changes (MNT-01).
- Key notation: `Ctrl+E` means keys pressed together; `gt` means keys pressed one after the other; `Ctrl+\` `n` means `Ctrl+\` followed by `n`. On macOS, `Ctrl` is the Control key (not Command) and `Alt` is the Option key.

### 1.5 Stakeholder requests

This specification is derived from the following requests. Appendix A traces each one to the requirements that address it.

1. Create an application like TeamViewer
2. Named TwoViewer
3. Animation
4. Audit logging
5. Dark / light / system theme, system by default
6. PostgreSQL, auto-migrate, no partition, ORM code-first
7. Simple architecture
8. Software as a service, free, no tenant, users
9. Splash screen
10. Usage via browser
11. Vim mode when remote
12. VPN

---

## 2. Overall description

### 2.1 Product perspective

TwoViewer is a new, self-contained product made of four parts: a **web client** in the user's browser, one hosted **server application**, a **PostgreSQL** database, and a **Host Agent** on each computer that is accessed.

All usage happens in the browser. A browser cannot, however, inject keyboard and mouse input into the operating system or create network interfaces, so a computer that is to be controlled, or used as a VPN endpoint, must run the Host Agent. The Host Agent is deliberately headless: apart from its installer and the session indicator, it has no windows, dialogs, or menus, and everything a person does with it happens in the browser.

### 2.2 Architecture overview

The architecture is intentionally simple: one server application, one database, one agent.

```
+---------------------------+                      +---------------------------+
|  Controller's browser     |                      |  Remote device            |
|  (TwoViewer Web Client)   | <== E2E session ==>  |  (TwoViewer Host Agent)   |
+-------------+-------------+                      +-------------+-------------+
              | web client, API, session setup                   | outbound only
              v                                                  v
+------------------------------------------------------------------------------+
|               TwoViewer Server Application (single deployable)               |
|           (web delivery, API, session broker, relay, audit, jobs)            |
|                   (applies database migrations at startup)                   |
+--------------------------------------+---------------------------------------+
                                       |
                                       v
                             +-------------------+
                             |    PostgreSQL     |
                             | (only data store) |
                             +-------------------+
```

Browsers and Host Agents connect to the server only through encrypted, outbound connections. Session traffic (screen, input, clipboard, files, and VPN packets) is end-to-end encrypted between the two endpoints. It flows directly between them when the network allows and otherwise through the relay, which forwards it without being able to read it.

| Component | Responsibilities |
|---|---|
| Web client | All user interaction: splash screen, sign-up and sign-in, devices, connecting, the remote session view (including Vim mode), VPN control, audit log, settings, host page, operator console. |
| Server application | One deployable unit. Serves the web client and provides the API, authentication, device registry and presence, session brokering (codes, consent, connection setup), the relay, audit logging, background jobs (retention, cleanup), and automatic database migration at startup. |
| Host Agent | Runs on controlled devices, installed or as QuickShare: screen capture, input injection, clipboard and file access, VPN endpoint, session indicator. Uses outbound connections only. |
| PostgreSQL | The only persistent store: accounts, preferences, devices, sessions, and audit events. |
| Email service (external) | Delivers verification, password-reset, and security-notification emails. |

### 2.3 Product functions

- **Accounts:** sign-up with email verification, sign-in with MFA (required for unattended access and VPN), password reset, sign-in management, account deletion.
- **Devices:** Host Agent download, pairing, presence, renaming, removal, unattended access per device.
- **Connecting:** unattended access to your own devices; attended support with a session code and consent.
- **Remote control:** viewing and controlling the remote screen, multiple monitors, special keys, clipboard, file transfer, view-only access, quality settings.
- **Vim mode:** modal, keyboard-driven control of remote sessions.
- **VPN:** point-to-point VPN between two of the user's devices.
- **Audit logging:** a complete trail of security and session events, visible to users (their own) and operators (all).
- **Appearance:** Light, Dark, and System themes (System by default), purposeful animation that respects reduced-motion preferences, and an animated splash screen.
- **Operations:** operator console for user suspension, abuse reports, and audit search.

### 2.4 User classes

| User class | Description | Main capabilities |
|---|---|---|
| Visitor | Anyone who is not signed in. | Sign up, sign in, reset a password, download QuickShare. |
| User | Holder of a verified account; every user can act as a controller. | Manage own devices; connect to own devices or with a session code; use VPN; manage settings; view own audit events. |
| Remote participant | The person at a device in an attended session; may or may not have an account. | On the host page: get a session code, allow or deny requests, follow and end sessions, report abuse. |
| Operator | TwoViewer staff, designated by server configuration. | Search users, suspend and reactivate accounts, handle abuse reports, search all audit events. |

### 2.5 Key user journeys

**J1. First use**

1. A visitor opens TwoViewer. The splash screen appears in the system theme, followed by the sign-in screen.
2. They sign up and verify their email address.
3. On *Add device*, they download and install the Host Agent. The agent opens the browser at the pairing page with a pairing code; the user confirms it, and the device appears online in the device list.
4. Optionally, they enable MFA and turn on unattended access for the device.

**J2. Unattended access to an own device**

1. From any browser, the user signs in and selects *Connect* on an online device that has unattended access turned on.
2. The session view opens, and the device shows the session indicator.
3. The user ends the session from the toolbar, or with `:q` in Vim mode. The start and end of the session are audited.

**J3. Attended support**

1. The helper, a signed-in user, asks the other person to download and run QuickShare from the TwoViewer website. QuickShare opens the host page in that person's browser, showing a session code.
2. The helper enters the code on the *Connect* screen. The host page shows the helper's name and verified email address with *Allow* and *Deny*.
3. After *Allow*, the session starts and the session indicator appears on the remote screen. Either side can end the session at any time, and the code cannot be reused.

**J4. VPN between own devices**

1. Two of the user's devices run the installed Host Agent, are online, and have unattended access turned on.
2. In the device list, the user chooses *Connect VPN* on one device and selects the other.
3. Each device gets a virtual IP address, shown in the web client. The devices can now reach each other with any network application, such as file sharing.
4. The user ends the VPN connection from the VPN page or from either device's session indicator.

### 2.6 Operating environment

| ID | Environment |
|---|---|
| OE-1 | TwoViewer is hosted and operated centrally; users never install or run server components. |
| OE-2 | Web client: the current and previous major versions of Chrome, Edge, Firefox, and Safari on desktop. All screens except the remote session view also work in the mobile versions of these browsers. |
| OE-3 | Host Agent: vendor-supported versions of Windows and macOS, and mainstream Linux distributions running a graphical desktop session. |
| OE-4 | Networks: home and office networks behind NAT and firewalls that allow outbound HTTPS. No inbound ports or port forwarding are needed. |

### 2.7 Design and implementation constraints

| ID | Constraint |
|---|---|
| CON-01 | **Simple architecture.** The server side shall be a single deployable application (a modular monolith) that provides web client delivery, API, session brokering, relay, audit logging, and background jobs. There shall be no separately deployed services. |
| CON-02 | PostgreSQL shall be the only stateful backing service. No message broker, cache server, search engine, or object storage shall be required. |
| CON-03 | The server application's internal modules (accounts, devices, sessions, VPN, audit, operations) shall have explicit boundaries in the code and communicate in-process. |
| CON-04 | **Usage via browser.** All user-facing functionality shall be delivered through the web client. The Host Agent shall show no windows, dialogs, or tray menus other than its installer, the session indicator, and prompts required by the operating system. |
| CON-05 | **Software as a service.** TwoViewer shall be offered only as a centrally hosted service; there is no self-hosted edition. |
| CON-06 | **Free.** There shall be no pricing, plans, trials, license keys, payments, or feature tiers. All features are available to every user, subject only to fair-use limits (AUTH-16). |
| CON-07 | **No tenants.** There shall be no organizations, teams, or workspaces, and the data model shall have no tenant concept. Every resource belongs to exactly one user account. |
| CON-08 | **Database.** PostgreSQL with a code-first ORM, automatic migrations at startup, and no table partitioning (§5.1). |
| CON-09 | No technology stack is prescribed beyond CON-08; technology choices are made during design. |
| CON-10 | The complete server side (application and database) shall be able to run on a single host. |

### 2.8 Assumptions and dependencies

| ID | Assumption |
|---|---|
| AS-01 | "Usage via browser" applies to the people using TwoViewer. Computers being controlled, and both ends of a VPN connection, run the Host Agent, because a browser cannot inject input into the operating system or create network interfaces. |
| AS-02 | "VPN" means a point-to-point VPN between two devices, as in TeamViewer. It is not a privacy VPN for internet traffic and does not give access to the remote device's whole local network. |
| AS-03 | "Vim mode when remote" means an optional, modal keyboard scheme for controlling remote sessions (§3.6). |
| AS-04 | "No tenant" means individual accounts only; nobody administers other users' accounts except operators. |
| AS-05 | In attended support, the controller must have an account; the remote participant does not need one. |
| AS-06 | Fair-use limits are sufficient to keep the free service sustainable. |

| ID | Dependency |
|---|---|
| DE-01 | An external email delivery service for transactional emails. |
| DE-02 | Code-signing certificates for Host Agent installers and updates on each supported operating system. |
| DE-03 | Operating-system support for screen capture, input injection, and virtual network adapters, including the permissions users must grant. |
| DE-04 | Hosting with enough bandwidth for relayed sessions at the capacity targets (§7.2). |

### 2.9 Out of scope for the first release

- Organizations, teams, device groups, and sharing devices between users.
- Paid plans, billing, and licensing.
- Native controller apps (desktop or mobile) and remote control from touch-only devices.
- Controlling phones and tablets.
- Session recording, chat, voice and video calls, remote printing, and Wake-on-LAN.
- VPN access to other devices on the remote network (subnet routing), full-tunnel internet VPN, and VPN with another person's device.
- Single sign-on and social login.
- Public API and third-party integrations.
- User interface languages other than English (the interface must be translatable later; A11Y-04).
- Resuming interrupted file transfers.

---

## 3. Functional requirements

Requirements are grouped by feature. Priorities follow §1.4.

### 3.1 Accounts and authentication

| ID | Requirement | Priority |
|---|---|---|
| AUTH-01 | A visitor shall be able to create an account with an email address, display name, and password, after accepting the Terms of Service and Privacy Notice. The accepted versions shall be recorded. | Must |
| AUTH-02 | The system shall verify the email address with a single-use link valid for 24 hours. Until it is verified, the user can sign in but cannot pair devices or start sessions. | Must |
| AUTH-03 | Passwords shall be at least 12 characters long. Passwords of at least 128 characters, containing any characters including spaces, shall be accepted. | Must |
| AUTH-04 | New passwords shall be rejected if they appear in a list of known compromised passwords. | Should |
| AUTH-05 | Users shall be able to sign in with email and password. If MFA is enabled, a second step shall require a valid authenticator code or an unused recovery code. | Must |
| AUTH-06 | Users shall be able to enable MFA with an authenticator app (time-based one-time codes). Enabling it shall issue 10 single-use recovery codes, shown once. | Must |
| AUTH-07 | Users shall be able to reset a forgotten password through an emailed single-use link valid for 1 hour. Completing a reset shall revoke all of the user's sign-in sessions. | Must |
| AUTH-08 | Failed sign-in attempts shall be throttled. After 10 failures for one account within 15 minutes, sign-in to that account shall be blocked for 15 minutes and the owner notified by email. | Must |
| AUTH-09 | Sign-in sessions shall expire after 7 days of inactivity and at most 30 days after sign-in. Users shall be able to list their active sign-ins (browser, operating system, IP address, last activity) and revoke any or all of them. | Must |
| AUTH-10 | These actions shall require re-authentication (password, plus second factor if enabled) when the last authentication is older than 15 minutes: changing password, email address, or MFA settings; turning on unattended access; deleting the account. | Must |
| AUTH-11 | Users shall be able to change their display name, their email address (effective only after the new address is verified; the old address is notified), and their password (current password required). | Must |
| AUTH-12 | The system shall email the user when their password is changed or reset, their email address is changed, MFA is enabled or disabled, a recovery code is used, unattended access is turned on for a device, or sign-in is blocked (AUTH-08). | Must |
| AUTH-13 | The system shall email the user when they sign in from a browser not used before on their account. | Should |
| AUTH-14 | Users shall be able to delete their account. Deletion shall immediately end their sessions, revoke all sign-ins and device credentials, and remove personal data as specified in PRIV-03. | Must |
| AUTH-15 | Sign-up shall be protected against automated abuse without third-party tracking, for example with a challenge shown after suspicious activity. | Should |
| AUTH-16 | Fair-use limits shall apply per user: at most 20 paired devices and 3 concurrent sessions (remote control and VPN combined). Reaching a limit shall show a clear explanation. | Must |
| AUTH-17 | Users shall only be able to see and act on their own resources: devices, sessions, VPN connections, preferences, and audit events. | Must |

### 3.2 Devices and Host Agent

| ID | Requirement | Priority |
|---|---|---|
| DEV-01 | The web client shall offer the Host Agent for every supported operating system, both as an installer (installed mode) and as a run-once program (QuickShare). QuickShare shall be downloadable without signing in. | Must |
| DEV-02 | In installed mode, the Host Agent shall run in the background, start with the operating system, and support attended sessions, unattended access, and VPN. Installation may require administrator rights. | Must |
| DEV-03 | QuickShare shall run without installation (and without administrator rights where the operating system allows), support attended sessions only, and leave nothing running or installed after it exits. It shall exit when the participant chooses *Quit* on the host page, or 15 minutes after its last session ended or its last session code expired. | Must |
| DEV-04 | On first start, an installed Host Agent shall open the device's default browser at the pairing page, which shows an 8-character pairing code valid for 15 minutes. A signed-in user with a verified email address shall confirm the code to pair the device with their account. | Must |
| DEV-05 | If no browser can be opened on the device, the Host Agent shall write the pairing address and code to its console and log, so pairing can be confirmed from another device. | Should |
| DEV-06 | Each paired device shall receive its own credential, used only by that device's Host Agent. Credentials shall be revocable and never shared between devices. | Must |
| DEV-07 | The device list shall show for each device: name, operating system, online or offline, last seen, unattended access on or off, Host Agent version, and current session, if any. Status changes shall appear within 10 seconds without reloading the page. | Must |
| DEV-08 | Users shall be able to rename and remove their devices. Removing a device shall immediately revoke its credential, end its sessions, and return its Host Agent to the unpaired state. | Must |
| DEV-09 | Unattended access shall be off by default for every device. Turning it on shall require MFA to be enabled on the account and re-authentication (AUTH-10). Disabling MFA shall turn off unattended access on all of the user's devices, after a warning. | Must |
| DEV-10 | The Host Agent shall connect to the server using outbound connections only, need no inbound ports or port forwarding, and reconnect automatically after interruptions (REL-05). | Must |
| DEV-11 | When the operating system requires permissions (for example, screen recording or input control) that have not been granted, the Host Agent shall report this, and the web client shall show the device as *Needs permission* with step-by-step guidance. | Must |
| DEV-12 | The Host Agent shall update itself automatically and verify the publisher's signature before applying an update. | Should |
| DEV-13 | The Host Agent shall work on networks that require an outbound proxy, using the operating system's proxy settings. | Should |
| DEV-14 | Uninstalling the Host Agent shall remove all of its components, including any virtual network adapter. The device stays listed as offline until the user removes it. | Must |

### 3.3 Connecting and session lifecycle

| ID | Requirement | Priority |
|---|---|---|
| SES-01 | Starting QuickShare, or launching TwoViewer on a device with the installed Host Agent, shall open the host page in the default browser. The host page shall show a 9-digit session code (displayed as `123 456 789`) that is valid for 10 minutes and for one session. The participant shall be able to generate a new code at any time, which invalidates the previous one. | Must |
| SES-02 | The link that opens the host page shall be bound to that Host Agent instance and stop working when the instance exits. | Must |
| SES-03 | A controller shall connect to an attended device by entering its session code on the *Connect* screen. | Must |
| SES-04 | The host page shall then show a consent prompt with the controller's display name and verified email address, the requested access (full control or view only), and the warning "Only allow access to people you know and trust. TwoViewer will never contact you to ask for access." The participant chooses *Allow* or *Deny*; no answer within 60 seconds counts as *Deny*. | Must |
| SES-05 | If the host page is not open when a connection request arrives, the Host Agent shall open it. | Must |
| SES-06 | A user shall be able to connect from the device list to their own online device with unattended access turned on, without a consent prompt. | Must |
| SES-07 | A device shall have at most one remote control session at a time. Further requests shall be rejected with the message "Device is busy". | Must |
| SES-08 | Sessions shall connect the endpoints directly when possible and fall back to the relay automatically. The transport in use (direct or relayed) shall be shown in the session information and recorded. | Must |
| SES-09 | A session shall end when the controller ends it (toolbar, or `:q` in Vim mode); when the participant ends it (host page or session indicator); when a timeout expires (SES-10, SES-11, VPN-07); when the device is removed; or when the controller's account is suspended or deleted. The end reason shall be recorded. | Must |
| SES-10 | A remote control session with no input from the controller for 60 minutes shall end automatically, after a warning shown 60 seconds beforehand that the controller can dismiss to continue. | Must |
| SES-11 | After a network interruption, a session shall reconnect automatically within 30 seconds without new consent; otherwise it shall end with the reason "Connection lost". | Must |
| SES-12 | Failed session-code entries shall be limited to 5 per controller account and 20 per IP address within 10 minutes. | Must |
| SES-13 | The participant shall be able to report abuse from the host page during or after a session; the report shall reach operators with a reference to the session. | Should |
| SES-14 | A participant without the Host Agent shall be able to open the host page directly and share their screen view-only from the browser, with a session code and consent prompt as in SES-01 and SES-04. The browser's own screen-sharing indicator takes the place of the session indicator. | Could |

### 3.4 Remote control

| ID | Requirement | Priority |
|---|---|---|
| RC-01 | The session view shall show the remote screen scaled to fit the browser window by default, with options for actual size (scrollable) and fullscreen. | Must |
| RC-02 | The controller shall be able to move the pointer, use the left, right, and middle buttons, double-click, drag, and scroll vertically and horizontally. | Must |
| RC-03 | While the session view has focus, every key the browser passes to the page, including modifier keys, shall be forwarded to the remote device unless Vim mode (§3.6) or the toolbar shortcut (RC-17) handles it. Combinations that the browser or local operating system keeps for itself are covered by RC-04 and RC-05. | Must |
| RC-04 | A *Send keys* menu shall send combinations that the local operating system or browser would otherwise intercept, at least Ctrl+Alt+Del, Alt+Tab, the Windows/Command key, and Print Screen. | Must |
| RC-05 | In fullscreen, the web client shall capture keys that the browser or operating system normally keeps for itself, such as Alt+Tab and `Esc`, and forward them to the remote device where the browser allows it. Where it does not, the web client shall say so when fullscreen starts, including that `Esc` then leaves fullscreen. | Should |
| RC-06 | The controller shall be able to switch between the remote device's monitors. | Must |
| RC-07 | The controller shall be able to show all remote monitors at once. | Should |
| RC-08 | Clipboard text shall be synchronized in both directions. Synchronization shall be on by default, switchable per session, and subject to the browser's permission. | Should |
| RC-09 | A session shall support view-only access, in which no input is sent. When view-only access was requested in an attended session, switching to full control shall require new consent. | Should |
| RC-10 | Quality shall default to *Auto*, which adapts resolution and frame rate to the available bandwidth. *Optimize speed* and *Optimize quality* shall also be available. | Must |
| RC-11 | The session toolbar shall provide: end session, monitor selection, scaling, fullscreen, send keys, file transfer, clipboard on/off, view-only, quality, Vim mode on/off, and session information (duration, direct or relayed, latency, frame rate). In fullscreen, the toolbar shall hide automatically and reappear when the pointer reaches the top edge or the toolbar shortcut (RC-17) is pressed. | Must |
| RC-12 | The session view shall show the device name and the connection state (connecting, connected, reconnecting, ended). | Must |
| RC-13 | The local pointer shall take the shape of the remote cursor. | Should |
| RC-14 | Text typed locally shall produce the same characters on the remote device even when the keyboard layouts differ. A *physical keys* option shall send key positions instead, for applications that need them. | Should |
| RC-15 | A user shall be able to run several sessions in separate browser tabs, up to the limit in AUTH-16. | Must |
| RC-16 | A per-device option shall lock the remote device's operating-system session when an unattended session ends. | Could |
| RC-17 | A *toolbar shortcut* shall show the session toolbar and move keyboard focus to it from the remote screen, in windowed and fullscreen views and in every Vim mode. Pressing it again, or completing a toolbar action, shall return focus to the remote screen. The default is `Ctrl+Alt+Shift`, pressed together and released without another key; users can change it in Settings. | Must |

### 3.5 File transfer

| ID | Requirement | Priority |
|---|---|---|
| FT-01 | The controller shall be able to upload files by dropping them on the session view or choosing them in a file dialog. Uploaded files shall be saved in a *TwoViewer* folder inside the remote user's Downloads folder; name conflicts shall be resolved by adding a number. | Should |
| FT-02 | The controller shall be able to browse the remote file system, with the permissions of the remote signed-in user, and download selected files through the browser. | Should |
| FT-03 | Transfers shall show progress and be cancellable. Failed or cancelled transfers shall leave no partial files. | Should |
| FT-04 | One operation may include several files; each file may be up to 2 GB. | Should |
| FT-05 | In attended sessions, the host page shall list each transfer with file name and direction. | Should |
| FT-06 | File transfer shall be unavailable in view-only sessions. | Should |

### 3.6 Vim mode

Vim mode makes remote control fully keyboard-driven. It applies only to the keyboard inside an active remote control session and, as in the Vim editor, uses modes to separate keys meant for TwoViewer from keys meant for the remote computer.

**Table 3.6-A — Modes**

| Mode | Status bar | Keyboard behavior |
|---|---|---|
| NORMAL | `-- NORMAL --` | Keys are TwoViewer commands (Table 3.6-B); nothing is sent to the remote device. |
| INSERT | `-- INSERT --` | All keys, including `Esc`, are sent to the remote device, except the escape sequence (VIM-04). |
| VISUAL | `-- VISUAL --` | The left mouse button is held down for dragging; motions move the pointer. |
| HINT | `-- HINT --` | A labeled grid covers the remote screen; typing a label moves the pointer there. |
| COMMAND-LINE | `:` | A command is being typed (Table 3.6-C). |

| ID | Requirement | Priority |
|---|---|---|
| VIM-01 | Vim mode shall be an optional per-user setting, *Vim mode in remote sessions*, off by default. The session toolbar shall also switch it on or off, and `:set novim` shall switch it off, for the current session only. | Must |
| VIM-02 | Vim mode shall affect only the keyboard inside an active remote control session view; the rest of the web client shall behave normally. Physical mouse input shall be forwarded in every mode. | Must |
| VIM-03 | When Vim mode is on, a session shall start in NORMAL mode. | Must |
| VIM-04 | In INSERT mode, the *escape sequence* shall return to NORMAL mode. The default is `Ctrl+\` followed by `n`, modeled on Vim's terminal-mode escape `Ctrl+\` `Ctrl+N`, whose second key browsers on Windows and Linux keep for opening a new window. If `Ctrl+\` is followed by another key, or by no key within 1,000 ms, the keys shall be sent to the remote device in the order pressed. `Esc` itself shall be sent to the remote device, so remote applications, including Vim running there, keep working. Users shall be able to change the escape sequence in Settings. | Must |
| VIM-05 | The current mode shall always be shown in the session status bar. Mode changes shall be animated (§4.4) and announced to assistive technologies. | Must |
| VIM-06 | NORMAL mode shall provide the default bindings in Table 3.6-B, with the priorities given there. | Must |
| VIM-07 | A numeric count before a motion or scroll shall repeat it; for example, `5j` moves the pointer five steps down. As in Vim, `0` is a motion unless it continues a count, as in `10j`. | Must |
| VIM-08 | Multi-key sequences such as `gt` shall time out after 1,000 ms, matching Vim's default. `Esc` shall cancel a pending count or sequence. | Must |
| VIM-09 | COMMAND-LINE mode shall provide the commands in Table 3.6-C. An unknown command shall show `E492: Not an editor command` in the status bar. | Must |
| VIM-10 | `:q` shall not end a session while a file transfer is running and shall show `E37: Transfer in progress (add ! to override)` instead. `:q!` shall end the session immediately. | Must |
| VIM-11 | COMMAND-LINE mode shall offer command history (Up and Down arrows) and Tab completion of command names. | Should |
| VIM-12 | In NORMAL, VISUAL, HINT, and COMMAND-LINE modes, keys shall not be sent to the remote device and shall not trigger browser actions, wherever the browser allows them to be suppressed. | Must |
| VIM-13 | `?` and `:help` shall show an overlay listing the active bindings and commands. | Must |
| VIM-14 | In HINT mode, grid labels shall be one or two letters, using home-row keys first; `Esc` shall cancel. | Should |
| VIM-15 | Users shall be able to remap NORMAL-mode bindings in Settings. Mappings that conflict with each other or with the escape sequence shall be rejected with an explanation, and *Reset to defaults* shall be available. | Should |
| VIM-16 | The pointer step (default 20 remote pixels) and the large step (default 10 steps) shall be adjustable in Settings. | Should |
| VIM-17 | Vim mode preferences (on or off, plus any customizations) shall be saved with the user's preferences and apply in every browser. | Must |

**Table 3.6-B — NORMAL mode default bindings**

| Keys | Action | Priority |
|---|---|---|
| `i`, `a` | Enter INSERT mode | Must |
| `h` `j` `k` `l` | Move the pointer one step left / down / up / right | Must |
| `w` / `b` | Move the pointer one large step right / left | Should |
| `}` / `{` | Move the pointer one large step down / up | Should |
| `0` / `$` | Move the pointer to the left / right edge of the current monitor | Must |
| `H` / `M` / `L` | Move the pointer to the top / middle / bottom of the current monitor | Must |
| `Space` | Left click | Must |
| `Enter` | Double-click | Must |
| `r` | Right click | Must |
| `v` | Enter VISUAL mode: press and hold the left button; `v` or `Esc` releases it | Must |
| `Ctrl+E` / `Ctrl+Y` | Scroll down / up one step | Must |
| `Ctrl+D` / `Ctrl+U` | Scroll down / up 5 steps | Must |
| `zh` / `zl` | Scroll left / right one step | Should |
| `gt` / `gT` | Show the next / previous monitor | Must |
| `y` | Yank: copy the remote selection to the local clipboard | Should |
| `p` | Put: paste the local clipboard into the remote application | Should |
| `f` | Enter HINT mode | Should |
| `:` | Enter COMMAND-LINE mode | Must |
| `?` | Show the help overlay | Must |
| `Esc` | Cancel a pending count or sequence; leave VISUAL or HINT mode | Must |

A scroll step is one mouse-wheel notch. `y` sends the remote operating system's copy shortcut and then copies the remote clipboard text to the local clipboard. `p` puts the local clipboard text on the remote clipboard and sends the remote paste shortcut. Both are unavailable when clipboard synchronization is off for the session.

**Table 3.6-C — Commands**

| Command | Action | Priority |
|---|---|---|
| `:q`, `:quit` | End the session (see VIM-10) | Must |
| `:q!` | End the session immediately | Must |
| `:mon {n}`, `:monitor {n}` | Show monitor *n* | Must |
| `:mon all` | Show all monitors | Should |
| `:send {keys}` | Send keys or key combinations, for example `:send ctrl+alt+del`, or `:send ctrl+\ ctrl+n` for Vim's terminal-mode escape on the remote device | Must |
| `:click {left\|right\|middle\|double}` | Click at the pointer position | Must |
| `:zoom {fit\|100}` | Fit to window / actual size | Should |
| `:fs`, `:fullscreen` | Toggle fullscreen | Should |
| `:set novim` | Turn Vim mode off for this session (turn it back on from the toolbar) | Must |
| `:set viewonly` / `:set noviewonly` | Turn view-only on / off (RC-09 applies) | Should |
| `:help` | Show the help overlay | Must |

### 3.7 VPN

| ID | Requirement | Priority |
|---|---|---|
| VPN-01 | A user shall be able to start a VPN connection from the web client between two of their own devices that are online and run the installed Host Agent: *Connect VPN* on one device, then select the other. Both devices shall have unattended access turned on (DEV-09). | Must |
| VPN-02 | Each endpoint shall receive a virtual IP address from a private address range reserved for TwoViewer and chosen to avoid ranges common in home and office networks. Addresses shall be unique within the user's account and shown in the web client. | Must |
| VPN-03 | The VPN shall be point-to-point: traffic shall be allowed only between the two virtual addresses. The Host Agent shall not route traffic to other networks, and the VPN shall not carry internet traffic. | Must |
| VPN-04 | VPN traffic shall be end-to-end encrypted between the two Host Agents and travel directly when possible, otherwise through the relay. Relayed VPN throughput shall be capped at 10 Mbit/s per connection. | Must |
| VPN-05 | While a VPN connection is active, both devices shall show the session indicator ("VPN connected to" followed by the peer device name) with a control to disconnect. | Must |
| VPN-06 | The VPN page shall list active connections (devices, virtual addresses, direct or relayed, duration, data transferred) and allow disconnecting any of them. | Must |
| VPN-07 | A VPN connection shall end when either side disconnects it, when either device is unreachable for more than 30 seconds, or after 60 minutes without traffic. | Must |
| VPN-08 | If a device cannot create its virtual network adapter (for example, because of missing administrator rights), the web client shall show the reason and how to fix it. | Must |
| VPN-09 | A VPN connection and a remote control session may run at the same time between the same devices. | Should |
| VPN-10 | VPN connections shall count toward the concurrent session limit (AUTH-16). | Must |

### 3.8 Audit logging

Audit logging records who did what, when, from where, and with what result. It supports security investigations and shows users who accessed their account and devices.

| ID | Requirement | Priority |
|---|---|---|
| AUD-01 | The system shall record an audit event for every action in Table 3.8-A. | Must |
| AUD-02 | Each audit event shall contain: unique ID; timestamp (UTC, millisecond precision); actor type (user, remote participant, operator, Host Agent, or system); actor ID, if any; IP address; client description (browser and operating system, or Host Agent version); action; target type and ID; outcome (success or failure, with a reason code); correlation ID (request or session); and action-specific details. | Must |
| AUD-03 | For a state-changing operation, the audit event shall be written in the same database transaction as the change; if the event cannot be written, the operation shall fail. Events without a state change, such as a failed sign-in, shall be written before the response is returned. | Must |
| AUD-04 | Audit events shall be append-only: the system shall provide no way to edit or delete individual events. Only the retention job (RET-02) shall remove events, after their retention period. | Must |
| AUD-05 | The database shall additionally reject updates to stored audit events, for example through a constraint or permission created by a migration. | Should |
| AUD-06 | Audit events shall never contain passwords, codes (session, pairing, MFA, recovery), tokens, keys, screen content, keystrokes, clipboard content, file content, or VPN traffic. File names are allowed. | Must |
| AUD-07 | Users shall be able to view the audit events in which they are the actor, or whose target is their account, one of their devices, or a session they took part in, newest first, filtered by time range, category, and outcome, with paging. IP addresses and client descriptions of other parties, and operator view and search events, shall not be shown. | Must |
| AUD-08 | Users shall be able to export their filtered audit events as CSV. | Should |
| AUD-09 | Operators shall be able to search all audit events by user (email address or ID), device, IP address, action, outcome, and time range. | Must |
| AUD-10 | Times shall be shown in the viewer's time zone, with the exact UTC timestamp available in the event details. | Must |
| AUD-11 | Actions by remote participants without an account shall be recorded with the Host Agent instance ID and IP address as the actor. | Must |
| AUD-12 | Each audit event shall store a hash linking it to the previous event, and a verification function shall report any break in the chain. | Could |
| AUD-13 | Changes to appearance preferences (theme, motion), Vim mode settings, and keyboard shortcuts shall not be audited. | Must |

**Table 3.8-A — Audited actions**

| Category | Actions |
|---|---|
| Authentication | sign-up; email verified; sign-in succeeded, failed, or blocked; sign-out; sign-in session revoked; re-authentication; password changed; password reset requested and completed; MFA enabled or disabled; recovery code used |
| Account | display name changed; email change requested and completed; terms accepted; personal data downloaded; account deleted |
| Devices | pairing started; device paired; device renamed; unattended access turned on or off; device removed; device credential revoked; Host Agent version changed |
| Sessions | session code generated; connection requested; consent granted, denied, or expired; session started (type, mode, access, transport); access changed (view-only or full control); session reconnected; session ended (reason, duration); abuse reported |
| File transfer | transfer started, completed, failed, or cancelled (direction, file name, size) |
| VPN | VPN started (devices, virtual addresses, transport); VPN ended (reason, duration, data transferred) |
| Operations | user details viewed; audit events searched; user suspended or reactivated (with reason); abuse report resolved; maintenance message published or removed |
| System | service started (version); migrations applied (identifiers); migration failed; retention job run (rows removed) |

### 3.9 Operator console

| ID | Requirement | Priority |
|---|---|---|
| OPS-01 | Operator rights shall be granted only through server configuration, never through the web client. Operators shall be required to use MFA. | Must |
| OPS-02 | Operators shall be able to search users by email address and see account status, creation date, last sign-in, number of devices, and active sessions. | Must |
| OPS-03 | Operators shall be able to suspend and reactivate a user, stating a reason. Suspension shall immediately end the user's sessions and VPN connections, revoke their sign-ins, and block sign-in with a message explaining how to contact support. | Must |
| OPS-04 | Operators shall be able to search all audit events (AUD-09). | Must |
| OPS-05 | Operators shall be able to review abuse reports, with links to the related audit events, and mark them resolved with a note. | Should |
| OPS-06 | Operators shall not be able to impersonate users, start sessions to users' devices, or access session content. | Must |
| OPS-07 | Every operator action, including viewing a user's details or audit events, shall be audited. | Must |
| OPS-08 | Operators shall be able to publish a short maintenance message shown to all signed-in users. | Could |

---

## 4. User interface requirements

### 4.1 Screens

| Screen | Used by | Purpose |
|---|---|---|
| Splash | Everyone | Shown while the web client loads (§4.2). |
| Sign in, Sign up, Verify email, Reset password | Visitors | Account access (§3.1). |
| Devices (home) | Users | Device list and status; *Connect*, *Connect VPN*, unattended access, rename, remove. |
| Add device | Users | Host Agent download and pairing instructions. |
| Pairing confirmation | Users | Confirm a Host Agent's pairing code. |
| Connect | Users | Enter a session code. |
| Remote session | Users | Remote screen, toolbar, status bar (including Vim mode), file transfer panel. |
| VPN | Users | Active VPN connections. |
| Audit log | Users | Own audit events (§3.8). |
| Settings | Users | Appearance (theme, motion); Remote sessions (Vim mode, escape sequence and key mappings, toolbar shortcut, defaults); Security (password, MFA, sign-ins); Account (email address, name, data download, deletion). |
| Host page | Remote participants | Session code, consent prompt, session status, file transfers, end session, report abuse, quit. |
| Operator console | Operators | Users, audit search, abuse reports. |
| Error and offline pages | Everyone | Not found, server unavailable, offline. |

### 4.2 Splash screen

| ID | Requirement | Priority |
|---|---|---|
| SPL-01 | On every full page load of the web client, a splash screen with the TwoViewer logo and name shall be shown until the application is ready (code loaded, sign-in state restored, preferences applied). | Must |
| SPL-02 | The splash screen shall be the first content painted, before the rest of the application has loaded, and shall appear within 1 second of navigation under reference conditions (§7). | Must |
| SPL-03 | The splash screen shall use the resolved theme from its first frame (THM-05). | Must |
| SPL-04 | The splash screen shall play a short logo animation of at most 1 second, which may be cut short once the application is ready. It shall stay visible for at least 500 ms to avoid flicker, but no longer than needed after the application is ready, and then transition out in at most 300 ms. | Must |
| SPL-05 | If the application is not ready within 10 seconds, the splash screen shall show "Still loading…"; if loading fails, it shall show an error message with a *Retry* action. | Must |
| SPL-06 | The splash screen shall not appear during in-app navigation, theme changes, or when sessions start or end. | Must |
| SPL-07 | With reduced motion (ANI-04), the logo shall be shown without animation, and the exit shall be a short cross-fade or immediate. | Must |
| SPL-08 | The splash screen shall expose its loading status to assistive technologies and shall not hold keyboard focus. | Must |
| SPL-09 | The splash screen shall show the web client version in small text. | Could |

### 4.3 Theme

| ID | Requirement | Priority |
|---|---|---|
| THM-01 | The web client shall offer three appearance options: Light, Dark, and System. The default shall be System. | Must |
| THM-02 | With System, the web client shall follow the operating system's or browser's color-scheme preference and switch immediately, without reloading, when that preference changes. | Must |
| THM-03 | Changing the option shall apply immediately without reloading, with a cross-fade of at most 200 ms (none with reduced motion). | Must |
| THM-04 | The option shall be saved in the user's preferences and applied in every browser after sign-in. It shall also be remembered in the browser, so that pages shown before sign-in (splash screen, sign-in, host page) use the last option chosen there; if none is stored, System applies. | Must |
| THM-05 | The first painted frame, including the splash screen, shall use the resolved theme; a flash of the other theme is a defect. | Must |
| THM-06 | Every screen and component, including dialogs, toasts, the session toolbar and status bar, the hint overlay, and the operator console, shall support both themes. | Must |
| THM-07 | Text and essential interface elements shall meet WCAG 2.1 AA contrast requirements in both themes. | Must |
| THM-08 | The option shall be available in Settings → Appearance and in the user menu. | Must |
| THM-09 | The remote screen image shall never be altered by the theme. | Must |
| THM-10 | The session indicator on controlled devices shall follow the device's operating-system theme. | Should |
| THM-11 | Transactional emails shall be legible in email clients using light or dark mode. | Should |

### 4.4 Animation

| ID | Requirement | Priority |
|---|---|---|
| ANI-01 | The web client shall use animation for: the splash screen; transitions between screens; opening and closing dialogs, menus, and panels; toasts; connection progress (connecting, reconnecting); showing and hiding the session toolbar; Vim mode changes; device status changes; and theme changes. | Must |
| ANI-02 | Interface transitions shall last 100–300 ms (the splash animation up to 1 second) and use easing. No animation shall delay or block user input. | Must |
| ANI-03 | Animations shall run at 55 frames per second or more on reference hardware and shall not cause layout shifts. | Must |
| ANI-04 | When the operating system or browser requests reduced motion, non-essential motion shall be removed; necessary state changes shall use short fades or no animation, and progress shall be shown without large movement. | Must |
| ANI-05 | Settings → Appearance shall offer Motion: System (default), Full, and Reduced. System follows the operating-system preference. | Should |
| ANI-06 | During a session, nothing shall be animated over the remote screen except the Vim mode indicator, the hint overlay, the toolbar showing or hiding, and notifications. | Must |
| ANI-07 | No content shall flash more than three times per second (WCAG 2.3.1). | Must |
| ANI-08 | Loading lists (devices, audit log) shall show placeholder skeletons with a subtle shimmer, omitted under reduced motion. | Should |

### 4.5 General

| ID | Requirement | Priority |
|---|---|---|
| UI-01 | The product name "TwoViewer" and its logo shall be used consistently in the web client, host page, splash screen, emails, installers, the session indicator, and the Host Agent's name in the operating system. | Must |
| UI-02 | In-app navigation shall not reload the page. Device status, session state, and VPN state shall update in real time without manual refresh. | Must |
| UI-03 | Destructive actions (removing a device, deleting the account) shall require confirmation; deleting the account shall require typing the account's email address. | Must |
| UI-04 | Empty states shall explain the next step, for example "No devices yet. Add your first device." | Must |
| UI-05 | When the connection to the server is lost, a non-blocking banner shall appear and the client shall retry automatically. | Must |
| UI-06 | Error messages shall say what happened and what the user can do, without exposing internal details. | Must |
| UI-07 | All screens except the remote session view shall be usable from 360 px wide; the remote session view is designed for desktop windows 1024 px wide or more. | Must |
| UI-08 | Toasts shall confirm completed background actions (for example, device paired or transfer complete), disappear after 5 seconds, and pause while hovered or focused. | Should |

---

## 5. Data requirements

### 5.1 Database and migrations

| ID | Requirement | Priority |
|---|---|---|
| DB-01 | PostgreSQL shall be the only persistent data store; one database shall serve the whole service. | Must |
| DB-02 | **Code-first:** the data model shall be defined as entities in the application code through an ORM. This model is the single source of truth for the database schema. | Must |
| DB-03 | Schema changes shall be made only through migrations generated from changes to the code model and kept in source control with the code. Manual schema changes are not allowed in any environment. | Must |
| DB-04 | A migration may contain hand-written steps only for what the ORM cannot express (for example, AUD-05); such steps remain part of the same ordered migration sequence. | Should |
| DB-05 | **Auto-migrate:** on startup, the server application shall apply all pending migrations in order before accepting any traffic, and report itself as not ready until they are complete. | Must |
| DB-06 | Each migration shall run atomically. If a migration fails, it shall be rolled back, logged, and audited where possible, and the application shall exit without serving traffic. | Must |
| DB-07 | If several instances start at the same time, a database lock shall ensure that exactly one applies migrations while the others wait. | Must |
| DB-08 | Starting against an empty database shall create the complete schema through the same migrations. No demo data or default accounts shall be created. | Must |
| DB-09 | The application shall refuse to start if the database contains migrations it does not know, meaning the schema is newer than the application. | Must |
| DB-10 | Applied migrations shall be recorded in the database with identifier, time applied, and application version. | Must |
| DB-11 | **No partitioning:** no table shall be partitioned in any form, and data shall not be sharded across databases. Large tables, such as audit events, are kept fast with indexes and trimmed by batched retention deletes (RET-02). | Must |
| DB-12 | Migrations shall be forward-only; a faulty migration is corrected by a new one. Destructive changes (dropping or renaming columns or tables) shall be spread over two releases (expand, then contract), so the previous application version keeps working during a deployment. | Should |
| DB-13 | Integrity shall be enforced in the database through primary keys, foreign keys, unique constraints (including case-insensitive uniqueness of email addresses), and not-null constraints, all declared in the code model. | Must |
| DB-14 | All timestamps shall be stored in UTC. | Must |
| DB-15 | Identifiers that appear in URLs or API calls shall be non-sequential and hard to guess. | Must |
| DB-16 | Data access shall go through the ORM. Hand-written queries are allowed only where needed for performance and shall always be parameterized. | Should |

### 5.2 Logical data model

The entities below are logical and shall be implemented as code-first entities (DB-02). Attribute lists are indicative. No entity has a tenant attribute (CON-07).

| Entity | Purpose | Main attributes |
|---|---|---|
| User | Registered account | email address (unique, case-insensitive), email verified at, password hash, display name, status (active, suspended, deleted), accepted terms version, created at |
| UserPreferences | Per-user settings | theme (System, Light, Dark; default System), motion (System, Full, Reduced; default System), Vim mode (default off), Vim escape sequence, key mappings, and step sizes, toolbar shortcut, session defaults (quality, clipboard synchronization) |
| MfaFactor | Second factor | user, encrypted secret, confirmed at, recovery codes (hashed, with time used) |
| SignInSession | Browser sign-in | user, created at, last active at, expires at, IP address, client description, revoked at |
| EmailLink | Single-use email links | user, purpose (verify, reset, email change), token hash, expires at, used at |
| Device | Paired computer | owner, name, operating system, Host Agent version, unattended access, credential hash, last seen at |
| AgentInstance | A running Host Agent, installed or QuickShare | instance ID, mode (installed, QuickShare), device (if paired), Host Agent version, operating system, IP address, started at, last seen at, exited at |
| PairingRequest | Pairing in progress | code hash, Host Agent instance, expires at, confirmed by, confirmed at |
| SessionCode | Attended access code | code hash, Host Agent instance, expires at, used at |
| RemoteSession | Remote control or VPN session | type (control, VPN), mode (attended, unattended), controller, target device or Host Agent instance, peer device (VPN), access (full, view-only), transport (direct, relayed), started at, ended at, end reason, virtual addresses and data transferred (VPN) |
| AbuseReport | Report from a participant | session, reporter IP address, description, status, resolved by, resolution note |
| AuditEvent | Audit trail | as in AUD-02 |
| Migration history | Applied migrations | identifier, applied at, application version (DB-10) |

### 5.3 Data retention

| ID | Requirement | Priority |
|---|---|---|
| RET-01 | Data shall be deleted when its retention period in Table 5.3-A ends. | Must |
| RET-02 | A daily background job in the server application shall delete expired data in small batches, so that no long-running locks occur. Each run shall be audited with the number of rows removed. | Must |
| RET-03 | Database backups shall be kept for 14 days (REL-04), so deleted data disappears from backups within that period. | Must |

**Table 5.3-A — Retention periods**

| Data | Retention |
|---|---|
| Audit events | 365 days |
| Remote control and VPN session records | 365 days |
| Host Agent instance records | 365 days after the instance exits |
| Sign-in sessions | 30 days after expiry or revocation |
| Email links, pairing requests, session codes | 24 hours after use or expiry |
| Abuse reports | 365 days after resolution |
| Personal data of deleted accounts | Removed within 30 days (PRIV-03) |

---

## 6. External interface requirements

### 6.1 User interfaces

The web client is specified in §4. Apart from its installer, the Host Agent's only user interface is the session indicator.

| ID | Requirement | Priority |
|---|---|---|
| IF-01 | During every session, the Host Agent shall show an always-on-top session indicator on the controlled device that names the controller (or, for VPN, the peer device), the access type, and the elapsed time, with an *End* button. The local user can move it but not hide it while a session is active, and the controller cannot affect it. | Must |

### 6.2 Operating-system interfaces (Host Agent)

| ID | Requirement | Priority |
|---|---|---|
| IF-02 | The Host Agent shall capture the screens of all connected monitors. | Must |
| IF-03 | The Host Agent shall inject keyboard and mouse input, including the secure attention sequence (Ctrl+Alt+Del) where the operating system allows it in installed mode. | Must |
| IF-04 | The Host Agent shall read and write clipboard text. | Should |
| IF-05 | The Host Agent shall read and write files with the permissions of the signed-in operating-system user. | Should |
| IF-06 | In installed mode, the Host Agent shall create and remove a virtual network adapter for VPN connections. | Must |
| IF-07 | The Host Agent shall detect missing operating-system permissions and report them (DEV-11). | Must |

### 6.3 Communication interfaces

| ID | Requirement | Priority |
|---|---|---|
| IF-08 | All communication between browsers, Host Agents, and the server shall use TLS 1.2 or later (1.3 preferred) over the standard HTTPS port, so that it passes typical firewalls and proxies. | Must |
| IF-09 | Browsers and Host Agents shall keep a real-time, two-way connection to the server for presence, session setup, and notifications. | Must |
| IF-10 | Session traffic shall travel directly between the endpoints when possible, and otherwise through the relay, which shall be reachable over the standard HTTPS port. | Must |
| IF-11 | Devices shall never accept inbound connections from the internet (SEC-05). | Must |

### 6.4 External services

| ID | Requirement | Priority |
|---|---|---|
| IF-12 | Transactional emails shall be sent through an external email delivery service from a TwoViewer domain with sender authentication configured, so that they are not flagged as spam. | Must |

---

## 7. Non-functional requirements

Unless stated otherwise, targets apply under these **reference conditions**: the controller uses a supported browser on a desktop or laptop computer up to four years old; the remote device has one 1920 × 1080 monitor; both endpoints have broadband connections with at least 25 Mbit/s download and 5 Mbit/s upload, and at most 50 ms round-trip time between them and to the service.

### 7.1 Performance

| ID | Requirement | Priority |
|---|---|---|
| PERF-01 | The splash screen shall appear within 1 second of navigation, and the web client shall be ready for use within 3 seconds (75th percentile) for a returning signed-in user. | Must |
| PERF-02 | A session shall show the first remote image within 5 seconds (95th percentile) of consent or of *Connect* on a direct connection, and within 8 seconds when relayed. | Must |
| PERF-03 | Input latency, from a local key press or pointer movement to its visible effect in the remote image, shall not exceed the network round-trip time plus 100 ms at the 95th percentile. | Must |
| PERF-04 | The remote image shall reach at least 24 frames per second at 1920 × 1080 when 5 Mbit/s is available, and at least 10 frames per second at 1 Mbit/s with *Auto* quality. | Must |
| PERF-05 | A direct VPN connection shall reach at least 80% of the available bandwidth, up to 100 Mbit/s. | Should |
| PERF-06 | API requests outside sessions shall be answered within 300 ms (95th percentile, server side) at the capacity targets in §7.2. | Must |
| PERF-07 | The first page of audit events (AUD-07, AUD-09) shall load within 2 seconds (95th percentile) with a full retention period of data in the unpartitioned table (CAP-02). | Must |
| PERF-08 | An idle Host Agent shall use no more than 1% CPU on average and no more than 100 MB of memory. | Should |

### 7.2 Capacity

| ID | Requirement | Priority |
|---|---|---|
| CAP-01 | The first release shall support 10,000 registered users, 25,000 paired devices, 5,000 Host Agents online at once, and 300 concurrent sessions, of which up to 100 are relayed. | Must |
| CAP-02 | The audit event table shall hold up to 50 million events (one retention period at full load) while meeting PERF-07. | Must |
| CAP-03 | The server application shall be able to run as more than one instance behind a load balancer, without infrastructure beyond PostgreSQL. | Could |

### 7.3 Reliability and availability

| ID | Requirement | Priority |
|---|---|---|
| REL-01 | Sign-in, the device list, and session setup shall be available at least 99.5% of each month, excluding announced maintenance. | Must |
| REL-02 | A deployment, including automatic migrations, shall make the service unavailable for no more than 2 minutes. | Should |
| REL-03 | Established direct sessions shall survive a server restart of up to 2 minutes; their audit events shall be completed once the server is back. | Should |
| REL-04 | The database shall be backed up at least daily, with backups encrypted and kept for 14 days. Recovery point objective: 24 hours. Recovery time objective: 4 hours. Restores shall be tested at least quarterly. | Must |
| REL-05 | Web clients and Host Agents shall reconnect automatically after the server becomes unavailable, using randomized, increasing delays to avoid reconnection storms. | Must |

### 7.4 Security

| ID | Requirement | Priority |
|---|---|---|
| SEC-01 | All network traffic shall be encrypted (IF-08). Plain HTTP requests shall be redirected to HTTPS, and browsers shall be instructed to use HTTPS only. | Must |
| SEC-02 | Session content (screen, input, clipboard, files, VPN traffic) shall be end-to-end encrypted between the two endpoints with new keys for every session. The server application, including the relay, shall never hold or be able to derive session keys. | Must |
| SEC-03 | Attended sessions shall require explicit consent (SES-04). Unattended access shall only reach devices owned by the controller with unattended access turned on (DEV-09). Every session shall show the session indicator (IF-01), except browser-only sharing, where the browser's own indicator is shown (SES-14). | Must |
| SEC-04 | The relay shall forward only the traffic of sessions authorized by the server. | Must |
| SEC-05 | Host Agents shall not accept inbound connections from the network, except VPN traffic within the limits of VPN-03. | Must |
| SEC-06 | Host Agent installers and updates shall be code-signed, and the Host Agent shall verify signatures before installing updates. | Must |
| SEC-07 | Passwords shall be stored only as salted hashes produced by a slow, memory-hard password-hashing function. MFA secrets shall be encrypted at rest. Session codes, pairing codes, recovery codes, and email-link tokens shall be stored only as hashes. | Must |
| SEC-08 | Brute-force protection shall apply to sign-in (AUTH-08), session codes (SES-12), pairing codes, MFA codes, and password-reset requests. | Must |
| SEC-09 | Sign-in credentials in the browser shall be inaccessible to page scripts and protected against cross-site request forgery. | Must |
| SEC-10 | Other websites shall not be able to embed the web client in frames (protecting the consent prompt against clickjacking), and only scripts from the TwoViewer origin shall run in it. | Must |
| SEC-11 | Every request shall be authorized against resource ownership (AUTH-17), and automated tests shall attempt access to other users' resources. | Must |
| SEC-12 | The web client and server shall meet OWASP ASVS Level 2 and address the OWASP Top 10. | Must |
| SEC-13 | Secrets such as database credentials and signing keys shall never be stored in source control; they shall be provided at runtime. | Must |
| SEC-14 | Responses to sign-up, sign-in, and password-reset requests shall not reveal whether an email address is registered. | Must |
| SEC-15 | Builds shall scan third-party components for known vulnerabilities. | Should |
| SEC-16 | An independent penetration test shall be carried out before public launch. | Should |

### 7.5 Privacy

| ID | Requirement | Priority |
|---|---|---|
| PRIV-01 | The service shall never store screen images, keystrokes, clipboard content, file content, or VPN traffic. | Must |
| PRIV-02 | Personal data shall be limited to what the service needs to operate: account data and preferences; device names and operating systems; records of sign-ins, sessions, and audit events, including IP addresses, client descriptions, and file names; and abuse report texts. Phone numbers, postal addresses, payment details, and contacts shall not be collected. | Must |
| PRIV-03 | When an account is deleted, its personal data shall be removed within 30 days. Its audit events shall be kept for the rest of their retention period, with names and email addresses replaced by a pseudonymous identifier. | Must |
| PRIV-04 | The Terms of Service and Privacy Notice shall be linked from every page and accepted at sign-up (AUTH-01). | Must |
| PRIV-05 | The web client shall load no third-party tracking or advertising code. | Should |
| PRIV-06 | Users shall be able to download their personal data (account, devices, preferences, audit events) in a machine-readable format. | Should |

### 7.6 Accessibility and usability

| ID | Requirement | Priority |
|---|---|---|
| A11Y-01 | The web client shall meet WCAG 2.1 Level AA; the remote screen image is excluded. | Must |
| A11Y-02 | Everything outside the remote screen image, including the session toolbar, shall be operable by keyboard with a visible focus indicator, with or without Vim mode; the toolbar shortcut (RC-17) moves focus out of the remote screen. | Must |
| A11Y-03 | Status changes (connection state, Vim mode, consent requests, errors) shall be announced to screen readers. | Must |
| A11Y-04 | The user interface shall be in English, with all user-facing text kept separate from code so it can be translated later. Dates, times, and numbers shall follow the user's locale and time zone. | Must |
| A11Y-05 | In usability testing with at least 5 first-time users, at least 80% shall pair a device and start a session within 5 minutes without help. | Should |

### 7.7 Compatibility

| ID | Requirement | Priority |
|---|---|---|
| CMP-01 | The web client shall work in the browsers listed in OE-2. | Must |
| CMP-02 | The Host Agent shall work on the operating systems listed in OE-3. | Must |
| CMP-03 | The server shall support the current and the previous minor version of the Host Agent. Older agents shall be shown as *Update required* and refused sessions until updated. | Must |
| CMP-04 | Where a browser lacks a capability (for example, clipboard access or keyboard capture), the web client shall disable the affected feature with an explanation instead of failing silently. | Should |

### 7.8 Maintainability and operability

| ID | Requirement | Priority |
|---|---|---|
| MNT-01 | All limits, timeouts, retention periods, and other numeric defaults in this document shall be configurable by operators without code changes. | Must |
| MNT-02 | The server application shall offer liveness and readiness checks; readiness shall be reported only after migrations have completed and the database is reachable (DB-05). | Must |
| MNT-03 | The server application shall write structured operational logs, separate from audit events, with correlation IDs. Logs shall contain no secrets or session content. | Must |
| MNT-04 | The server application shall expose metrics for online Host Agents; active sessions by type (control or VPN) and transport (direct or relayed); relay bandwidth; failed sign-ins; error rates; and migration duration. | Must |
| MNT-05 | Automated tests shall run on every change and include applying all migrations to an empty database, upgrading from the previous release's schema, and checking that the code model and the migrations are in sync (no un-migrated model changes). | Must |
| MNT-06 | The server application and Host Agent shall use semantic versioning and publish release notes for every release. | Must |
| MNT-07 | A developer shall be able to start the complete server side (application and PostgreSQL) locally with a single command. | Should |

---

## 8. Open issues

| ID | Question | Current assumption |
|---|---|---|
| OI-1 | Should remote participants in attended sessions need an account? | No (AS-05). |
| OI-2 | Should Vim mode start in NORMAL or INSERT mode? | NORMAL (VIM-03). |
| OI-3 | Will the VPN later need access to other devices on the remote network? | Out of scope (VPN-03). |
| OI-4 | Are the fair-use limits and relay caps affordable at the capacity targets? | AUTH-16, VPN-04, CAP-01. |
| OI-5 | Is 365-day audit retention right for the legal requirements of the target markets? | RET-01. |
| OI-6 | Logo, color palette, and splash animation. | To be supplied by design. |
| OI-7 | Is `Ctrl+\` `n` a good default escape sequence, given that `\` is awkward to type on some keyboard layouts? | Yes, because users can change it (VIM-04). |

---

## Appendix A. Traceability

| Stakeholder request | Addressed by |
|---|---|
| Application like TeamViewer | §3.1–§3.5 (AUTH, DEV, SES, RC, FT), §3.9 (OPS) |
| Named TwoViewer | UI-01 |
| Animation | ANI-01–ANI-08, SPL-04, SPL-07, THM-03, VIM-05 |
| Audit logging | AUD-01–AUD-13, Table 3.8-A, OPS-07, RET-01–RET-02 |
| Dark / light / system, system by default | THM-01–THM-11, SPL-03 |
| PostgreSQL, auto-migrate, no partition, ORM code-first | CON-08, DB-01–DB-16, §5.2, PERF-07, CAP-02, MNT-02, MNT-05 |
| Simple architecture | §2.2, CON-01–CON-03, CON-10, MNT-07 |
| Software as a service, free, no tenant, users | CON-05–CON-07, AUTH-01–AUTH-17 |
| Splash screen | SPL-01–SPL-09, PERF-01 |
| Usage via browser | §2.1, CON-04, AS-01, OE-2, SES-14, CMP-01 |
| Vim mode when remote | VIM-01–VIM-17, Tables 3.6-A to 3.6-C, RC-17 |
| VPN | VPN-01–VPN-10, AS-02, IF-06 |
