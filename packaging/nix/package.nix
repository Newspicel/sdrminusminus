{
  lib,
  stdenv,
  rustPlatform,
  fetchPnpmDeps,
  pnpmConfigHook,
  pnpm,
  nodejs_26,
  pkg-config,
  cmake,
  copyDesktopItems,
  makeDesktopItem,
  wrapGAppsHook3,
  cairo,
  gdk-pixbuf,
  glib,
  gtk3,
  libayatana-appindicator,
  libGL,
  libopus,
  ffmpeg,
  librsvg,
  libsoup_3,
  openssl,
  pango,
  soapysdr,
  vulkan-loader,
  webkitgtk_4_1,
  xdotool,
  soapyPlugins ? [ ],
  sdrplayApi ? null,
}:

let
  runtimeLibraries = [
    libGL
    vulkan-loader
  ]
  ++ lib.optional (sdrplayApi != null) sdrplayApi;
in
rustPlatform.buildRustPackage (finalAttrs: {
  pname = "sdrmm-desktop";
  version = (builtins.fromTOML (builtins.readFile ../../Cargo.toml)).workspace.package.version;

  src = lib.cleanSource ../..;

  cargoLock = {
    lockFile = ../../Cargo.lock;
    outputHashes = {
      # git rev 096c805278faa0a904de7d98066bf4cf395cb6c2
      "xng-acars-0.21.0" = "sha256-O/+eP1Eyx5PGrP7+YGJmOIRFFwF2t9HOWtag5z1lUWM=";
    };
  };

  pnpmDeps = fetchPnpmDeps {
    inherit (finalAttrs) pname version src;
    inherit pnpm;
    sourceRoot = "${finalAttrs.src.name}/web";
    fetcherVersion = 4;
    # web/pnpm-lock.yaml sha256:9adfd09fc38885074ca79da9513aa9b640da0faa683f7c011872600874eec769
    hash =
      {
        aarch64-linux = "sha256-wgZEELC/UL3KfFdiIHMl19UmtOWrTSaOiwNOklL/sJY=";
        x86_64-linux = "sha256-MMmC+DihKSdmq5ZnxZlcXLNc3aJBBDhxtQur1R+8kBo=";
      }
      .${stdenv.hostPlatform.system};
  };
  pnpmRoot = "web";

  nativeBuildInputs = [
    cmake
    rustPlatform.bindgenHook
    copyDesktopItems
    nodejs_26
    pkg-config
    pnpm
    pnpmConfigHook
    wrapGAppsHook3
  ];

  buildInputs = [
    cairo
    gdk-pixbuf
    glib
    gtk3
    libayatana-appindicator
    libopus
    ffmpeg
    librsvg
    libsoup_3
    openssl
    pango
    webkitgtk_4_1
    xdotool
  ];

  preBuild = ''
    find web/node_modules -path '*/.bin/*' -type f -exec sed -i 's/command -p /command /g' {} +
    pnpm --dir web build
  '';

  cargoBuildFlags = [
    "--package"
    "sdrmm-desktop"
  ];
  cargoTestFlags = finalAttrs.cargoBuildFlags;

  desktopItems = [
    (makeDesktopItem {
      name = "sdrmm-desktop";
      desktopName = "SDR--";
      comment = "Software-defined radio receiver";
      exec = "sdrmm-desktop";
      icon = "dev.newspicel.sdrmm";
      categories = [
        "AudioVideo"
        "HamRadio"
      ];
    })
  ];

  postInstall = ''
    install -Dm644 apps/desktop/icons/128x128.png \
      "$out/share/icons/hicolor/128x128/apps/dev.newspicel.sdrmm.png"
    install -Dm644 apps/desktop/icons/128x128@2x.png \
      "$out/share/icons/hicolor/256x256/apps/dev.newspicel.sdrmm.png"
  '';

  # Nothing links SoapySDR: it is opened at runtime, and outside a Nix store there is no
  # default path to find it on. The wrapper names the store copy, and the plugins the user
  # selected stay separate packages it merely points at.
  preFixup = ''
    gappsWrapperArgs+=(
      --set-default SDRMM_SOAPY_LIBRARY "${soapysdr}/lib/libSoapySDR${stdenv.hostPlatform.extensions.sharedLibrary}"
      --prefix LD_LIBRARY_PATH : "${lib.makeLibraryPath runtimeLibraries}"
    )
  '' + lib.optionalString (soapyPlugins != [ ]) ''
    gappsWrapperArgs+=(
      --prefix SOAPY_SDR_PLUGIN_PATH : "${lib.makeSearchPath soapysdr.searchPath soapyPlugins}"
    )
  '';

  passthru = {
    inherit soapyPlugins sdrplayApi;
  };

  meta = {
    description = "Modular software-defined radio receiver desktop application";
    homepage = "https://sdrmm.com";
    license = lib.licenses.agpl3Plus;
    mainProgram = "sdrmm-desktop";
    platforms = lib.platforms.linux;
  };
})
