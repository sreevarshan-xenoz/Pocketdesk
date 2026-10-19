// Pocketdesk: send to phone (ecosystem phase 1d). Right-click a page, a
// link or an image; the request goes to Pocketdesk on this computer through
// native messaging (`pocketdesk browser-host`), which sends it to the phone.

const HOST = "dev.pocketdesk.host";

const MENUS = [
  { id: "page", title: "Send page to phone", contexts: ["page"] },
  { id: "continue", title: "Continue reading on phone", contexts: ["page"] },
  { id: "link", title: "Send link to phone", contexts: ["link"] },
  { id: "image", title: "Send image to phone", contexts: ["image"] },
];

chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.removeAll(() => {
    for (const menu of MENUS) chrome.contextMenus.create(menu);
  });
});

/** What a menu click asks Pocketdesk for, or null when there is nothing to send. */
function request(menuItemId, info, tab) {
  switch (menuItemId) {
    case "page":
      return tab && tab.url ? { send: "link", url: tab.url } : null;
    case "continue":
      return tab && tab.url ? { send: "link", url: tab.url, continueFrom: tab.id } : null;
    case "link":
      return info.linkUrl ? { send: "link", url: info.linkUrl } : null;
    case "image":
      return info.srcUrl ? { send: "image", url: info.srcUrl } : null;
    default:
      return null;
  }
}

// D6: the longest link Pocketdesk sends (core::protocol::link).
const MAX_LINK = 2048;

/**
 * Runs in the page: the first words of the first paragraph or heading whose
 * top is in the window, the place to continue from; "" when there is none.
 */
function topText() {
  const blocks = document.querySelectorAll("p, h1, h2, h3, h4, h5, h6, li, blockquote, pre");
  for (const block of blocks) {
    const box = block.getBoundingClientRect();
    if (box.top < 0 || box.top >= window.innerHeight || box.height === 0) continue;
    const words = (block.innerText || "").trim().split(/\s+/).filter(Boolean);
    if (words.length >= 3) return words.slice(0, 6).join(" ");
  }
  return "";
}

/**
 * `url` with a text fragment for `snippet`, so the phone's browser scrolls to
 * it; `url` as it is with no snippet, or when the link would be too long.
 * A text fragment the page already had is replaced.
 */
function continueUrl(url, snippet) {
  if (!snippet) return url;
  const hash = url.indexOf("#");
  const base = hash < 0 ? url : url.slice(0, hash);
  const fragment = hash < 0 ? "" : url.slice(hash + 1).split(":~:")[0];
  // "-", "," and "&" separate a text directive's parts: encoded, they are text.
  const text = encodeURIComponent(snippet).replace(/-/g, "%2D").replace(/,/g, "%2C");
  const out = `${base}#${fragment}:~:text=${text}`;
  return out.length <= MAX_LINK ? out : url;
}

function tell(message) {
  chrome.notifications.create({
    type: "basic",
    iconUrl: "icon-128.png",
    title: "Pocketdesk",
    message,
  });
}

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  const asked = request(info.menuItemId, info, tab);
  if (!asked) return;
  if (asked.continueFrom !== undefined) {
    // Where the page is now; a page that cannot be read (the browser's own,
    // a store) goes as it is.
    try {
      const [ran] = await chrome.scripting.executeScript({
        target: { tabId: asked.continueFrom },
        func: topText,
      });
      asked.url = continueUrl(asked.url, ran && ran.result);
    } catch (e) {
      // Sent without the place.
    }
    delete asked.continueFrom;
  }
  if (asked.url.startsWith("data:") || asked.url.startsWith("blob:")) {
    tell("This one lives only in the page: save it, then send the file.");
    return;
  }
  let answer;
  try {
    answer = await chrome.runtime.sendNativeMessage(HOST, asked);
  } catch (e) {
    const why = String((e && e.message) || e);
    // Chromium: "Specified native messaging host not found."; Firefox:
    // "No such native application …". Anything else: the host ran and failed.
    if (/not found|no such native application/i.test(why)) {
      tell(
        "Pocketdesk is not set up for this browser: run `pocketdesk browser-host --install` " +
          '(or Settings, "Send from the browser", in the Pocketdesk app).',
      );
    } else {
      tell("Pocketdesk could not answer: " + why);
    }
    return;
  }
  tell((answer && (answer.ok || answer.error)) || "No answer from Pocketdesk.");
});
