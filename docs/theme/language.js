(function () {
  "use strict";

  const EN = "en";
  const ZH = "zh";

  const pages = {
    "en/index.html": { lang: EN, other: "README.zh.html", label: "文", title: "切换到中文" },
    "ROADMAP.html": { lang: EN, other: "ROADMAP.zh.html", label: "文", title: "切换到中文" },
    "en/AGENT.html": { lang: EN, other: "AGENT.zh.html", label: "文", title: "切换到中文" },
    "README.zh.html": { lang: ZH, other: "en/index.html", label: "A", title: "Switch to English" },
    "ROADMAP.zh.html": { lang: ZH, other: "ROADMAP.html", label: "A", title: "Switch to English" },
    "AGENT.zh.html": { lang: ZH, other: "en/AGENT.html", label: "A", title: "Switch to English" },
  };

  function currentPage() {
    return normalizedPath(window.location.pathname);
  }

  function normalizedPath(pathname) {
    const hasTrailingSlash = pathname.endsWith("/");
    const parts = pathname.split("/").filter(Boolean);
    let path = parts.join("/");
    if (path === "") {
      return "index.html";
    }
    const bookIdx = parts.lastIndexOf("book");
    if (bookIdx >= 0) {
      path = parts.slice(bookIdx + 1).join("/");
    } else if (parts.length > 2) {
      const idx = parts.lastIndexOf("sift");
      if (idx >= 0) {
        path = parts.slice(idx + 1).join("/");
      } else {
        path = parts.slice(-2).join("/");
      }
    } else {
      const idx = parts.lastIndexOf("sift");
      if (idx >= 0) {
        path = parts.slice(idx + 1).join("/");
      }
    }
    if (path === "") {
      return "index.html";
    }
    if (hasTrailingSlash || !path.split("/").pop().includes(".")) {
      return path.replace(/\/$/, "") + "/index.html";
    }
    return path;
  }

  function rootPrefix() {
    const page = currentPage();
    return page.includes("/") ? "../" : "";
  }

  function addLanguageLinks() {
    const rightButtons = document.querySelector("#mdbook-menu-bar .right-buttons");
    if (!rightButtons || document.querySelector(".sift-language-switch")) {
      return;
    }

    const page = currentPage();
    if (page === "index.html") {
      rightButtons.prepend(makeLink(rootPrefix() + "README.zh.html", "文", "中文"));
      rightButtons.prepend(makeLink(rootPrefix() + "en/index.html", "A", "English"));
      return;
    }

    const meta = pages[page];
    if (!meta) {
      return;
    }
    rightButtons.prepend(makeLink(rootPrefix() + meta.other, meta.label, meta.title));
  }

  function makeLink(href, label, title) {
    const link = document.createElement("a");
    link.href = href;
    link.className = "sift-language-switch";
    link.title = title;
    link.setAttribute("aria-label", title);
    link.textContent = label;
    return link;
  }

  function filterSidebar() {
    const sidebar = document.querySelector("#mdbook-sidebar mdbook-sidebar-scrollbox");
    if (!sidebar || sidebar.querySelectorAll("ol.chapter > li").length === 0) {
      return false;
    }

    const page = currentPage();
    const lang = pages[page] ? pages[page].lang : null;
    const topItems = Array.from(sidebar.querySelectorAll("ol.chapter > li.chapter-item"));
    for (const item of topItems) {
      const firstLink = item.querySelector(":scope > .chapter-link-wrapper > a");
      if (!firstLink) {
        continue;
      }
      const href = normalizedHref(firstLink);
      const itemLang = languageForHref(href);
      if (!lang) {
        item.hidden = itemLang !== null;
      } else {
        item.hidden = itemLang !== null && itemLang !== lang;
      }
    }

    const home = topItems
      .map((item) => item.querySelector(":scope > .chapter-link-wrapper > a"))
      .find((link) => link && normalizedHref(link) === "index.html");
    if (home) {
      home.textContent = lang === ZH ? "首页" : "Home";
      if (!lang) {
        home
          .closest("li.chapter-item")
          ?.querySelectorAll(":scope > .on-this-page")
          .forEach((section) => {
            section.hidden = true;
          });
      }
    }
    return true;
  }

  function normalizedHref(link) {
    const href = link.getAttribute("href") || "";
    const url = new URL(href, window.location.href);
    return normalizedPath(url.pathname);
  }

  function languageForHref(href) {
    if (href === "README.zh.html" || href === "ROADMAP.zh.html" || href === "AGENT.zh.html") {
      return ZH;
    }
    if (href === "en/index.html" || href === "ROADMAP.html" || href === "en/AGENT.html") {
      return EN;
    }
    return null;
  }

  function init() {
    addLanguageLinks();
    if (filterSidebar()) {
      return;
    }
    let attempts = 0;
    const timer = window.setInterval(function () {
      attempts += 1;
      if (filterSidebar() || attempts > 20) {
        window.clearInterval(timer);
      }
    }, 25);
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
})();
