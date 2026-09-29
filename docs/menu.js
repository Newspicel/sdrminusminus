const title = document.querySelector(".menu-title");
if (title !== null) {
  const home = document.createElement("a");
  home.href = "/";
  home.className = "menu-home";
  const mark = document.createElement("img");
  mark.src = "/icon.svg";
  mark.alt = "";
  mark.width = 22;
  mark.height = 22;
  home.append(mark, title.textContent ?? "");
  title.replaceChildren(home);
}

const buttons = document.querySelector(".right-buttons");
if (buttons !== null) {
  const download = document.createElement("a");
  download.href = "/download";
  download.className = "menu-download";
  download.textContent = "Download";
  buttons.prepend(download);
}
