// Pocketdesk: send to phone (ecosystem phase 1d). Right-click a page, a
// link or an image; the request goes to Pocketdesk on this computer through
// native messaging (`pocketdesk browser-host`), which sends it to the phone.

const HOST = "dev.pocketdesk.host";

const MENUS = [
  { id: "page", title: "Send page to phone", contexts: ["page"] },
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
    case "link":
      return info.linkUrl ? { send: "link", url: info.linkUrl } : null;
    case "image":
      return info.srcUrl ? { send: "image", url: info.srcUrl } : null;
    default:
      return null;
  }
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
