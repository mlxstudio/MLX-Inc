Windows application and installer branding assets:

  icon.png                 Source mark used to generate the app icons
  icon-source.svg          Supplied app icon artwork
  feather-source.svg       Supplied teal feather artwork
  32x32.png, 128x128.png,
  128x128@2x.png, icon.ico  Icons used by the app and Windows executable
  installer-header.bmp     NSIS installer and uninstaller header artwork (150 x 57)
  installer-sidebar.bmp    NSIS welcome and finish artwork (164 x 314)
  ../nsis/installer.nsi    Branded installer theme and explicit uninstaller icon

To regenerate the app icons from a square PNG:
  npx tauri icon path\to\your-logo.png --output src-tauri\icons

If the mark changes, also update the NSIS header and sidebar bitmaps to match.
