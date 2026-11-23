# Installing and removing Ailfred

Ailfred needs macOS 14 or later. It runs in the menu bar only (no Dock icon until the chat gets
its own window).

## From source (this Mac)

1. Put Rune beside this repository, as `../_references/rune`, so the folders look like this:

   ```
   <folder>/ailfred-app        this repository
   <folder>/_references/rune   https://github.com/anomalyco/rune (last checked with
                                21e5342b9a4cc33eeee88071b4b1f0eafb1247d4)
   ```

2. Install the Rust toolchain in `rust-toolchain.toml` (rustup picks it up by itself).
3. Once per Mac, make the signing identity:

   ```sh
   scripts/make-signing-identity
   ```

   It adds a self-signed certificate, "Ailfred Local Signing", to your login keychain. Signing
   every build with it keeps the Screen Recording permission across reinstalls. Without it the app
   is signed ad hoc, and macOS asks for Screen Recording again after each install.
4. Build, sign, install and open the app:

   ```sh
   scripts/install-mac
   ```

   This installs `~/Applications/Ailfred.app`, replacing the copy that's there and opening the new one.
5. Turn on Ailfred under **System Settings › Privacy & Security › Screen & System Audio
   Recording** when asked (Settings › Permissions shows whether it's on, with a button there),
   then quit and reopen it.

`cargo run` runs a development build. It keeps its own settings (`com.ailfred.dev`), but it shares
the global shortcuts with the installed app, so quit one before starting the other.

## A disk image for another Mac

```sh
scripts/make-dmg
```

This writes `dist/Ailfred-<version>-<build>.dmg`. It holds the app and a link to Applications,
so the user drags the app onto the link to install it.

The image is signed with the local identity, not notarized, so another Mac reports an
unidentified developer the first time. To open it anyway, Control-click Ailfred in Applications
and choose **Open**, or allow it under **System Settings › Privacy & Security**. That Mac then
asks for Screen Recording itself.

## Shortcuts and links

- **⇧⌘Space** captures an area of the screen.
- **⌃⌥Space** opens the quick input.

Both shortcuts can be changed in Settings. If a saved shortcut is taken by another app when
Ailfred starts, Ailfred uses that shortcut's default instead, and Settings says so.

`open ailfred://toggle` shows or hides the quick input from a script, Shortcuts or a launcher.

## Removing Ailfred

1. In Ailfred's menu, turn off **Launch at Login**, then choose **Quit Ailfred**.
2. Delete the app:

   ```sh
   rm -rf ~/Applications/Ailfred.app
   ```

3. Delete its data: settings, saved conversations, Ailfred's own pi setup and the Claude Code
   folder, plus the log:

   ```sh
   rm -rf ~/Library/Application\ Support/com.ailfred ~/Library/Application\ Support/com.ailfred.dev
   rm -f ~/Library/Logs/Ailfred.log
   ```

4. Remove the Anthropic API key from the Keychain, if you saved one:

   ```sh
   security delete-generic-password -s com.ailfred -a anthropic-api-key
   ```

5. Take back the Screen Recording permission:

   ```sh
   tccutil reset ScreenCapture com.ailfred
   ```

6. Optional: delete the signing identity. In Keychain Access, find "Ailfred Local Signing" under
   login › My Certificates, then delete both the certificate and its key.

Screenshots stay where they were saved (Desktop, unless you chose another folder in Settings).
