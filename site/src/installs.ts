export interface Install {
  id: string;
  tab: string;
  title: string;
  note: string;
  lines: string;
  more?: { href: string; label: string };
}

export const INSTALLS: Install[] = [
  {
    id: "brew",
    tab: "Homebrew",
    title: "Homebrew",
    note: "The app on macOS. The server on macOS or Linux.",
    lines: `brew install newspicel/tap/sdrmm-app

<span class="p">or the server</span>
brew install sdrmm
brew services start sdrmm`,
  },
  {
    id: "apt",
    tab: "APT",
    title: "Debian and Ubuntu",
    note: "Signed APT repository for x86-64 and ARM64.",
    lines: `curl -fsSL https://downloads.sdrmm.com/packages/key.gpg \\
  | sudo tee /usr/share/keyrings/sdrmm.gpg > /dev/null
echo "deb [signed-by=/usr/share/keyrings/sdrmm.gpg] \\
https://downloads.sdrmm.com/packages/deb stable main" \\
  | sudo tee /etc/apt/sources.list.d/sdrmm.list
sudo apt update
sudo apt install sdrmm-app`,
  },
  {
    id: "dnf",
    tab: "DNF",
    title: "Fedora",
    note: "Signed RPM repository for x86-64 and ARM64.",
    lines: `sudo dnf config-manager addrepo \\
  --from-repofile=https://downloads.sdrmm.com/packages/rpm/sdrmm.repo
sudo dnf install sdrmm-app`,
  },
  {
    id: "nix",
    tab: "Nix",
    title: "Nix",
    note: "Flake package for x86-64 and ARM64 Linux, with SoapySDR modules selectable.",
    lines: `nix --extra-experimental-features 'nix-command flakes' \\
  profile install github:Newspicel/sdrmm
sdrmm-desktop`,
    more: { href: "/docs/getting-started/install#nix", label: "NixOS module options" },
  },
  {
    id: "container",
    tab: "Container",
    title: "Container",
    note: "Keeps its database and recordings in a volume. Pass through the USB group that owns your radio.",
    lines: `git clone https://github.com/Newspicel/sdrmm.git
cd sdrmm
docker compose up -d`,
    more: { href: "/docs/server/deployment", label: "Container setup, auth and HTTPS" },
  },
];

export function installFor(agent: string): string {
  if (/Android|iPhone|iPad/.test(agent)) {
    return "brew";
  }
  if (/Windows/.test(agent)) {
    return "container";
  }
  if (/Fedora|Red Hat|CentOS|Rocky|AlmaLinux|SUSE/i.test(agent)) {
    return "dnf";
  }
  if (/Linux|X11|CrOS/.test(agent)) {
    return "apt";
  }
  return "brew";
}
