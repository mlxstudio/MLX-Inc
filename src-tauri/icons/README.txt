Windows application and installer branding assets:

  icon.png                 Source mark used to generate the app icons
  icon-source.svg          Supplied app icon artwork
  feather-source.svg       Supplied teal feather artwork
  32x32.png, 128x128.png,
  128x128@2x.png, icon.ico  Icons used by the app and Windows executable
  installer-header.svg     Source artwork for the NSIS header
  installer-header.bmp     NSIS installer and uninstaller header artwork (150 x 57)
  installer-sidebar.svg    Source artwork for the NSIS welcome and finish panel
  installer-sidebar.bmp    NSIS welcome and finish artwork (164 x 314)
  ../nsis/installer.nsi    Branded installer theme and explicit uninstaller icon

To regenerate the app icons from a square PNG:
  npx tauri icon path\to\your-logo.png --output src-tauri\icons

The installer SVGs are rendered to 4x PNGs and downsampled to the NSIS bitmap
sizes. If the mark changes, update the SVGs and regenerate both bitmaps.
