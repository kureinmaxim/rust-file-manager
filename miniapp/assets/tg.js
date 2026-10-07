// Thin wrapper over Telegram.WebApp. Every feature is gated by the client's
// Bot API version (isVersionAtLeast); outside Telegram the app degrades to
// in-page buttons and browser dialogs so it can still be developed and tested.

const W = window.Telegram && window.Telegram.WebApp;
const at = (version) => Boolean(W && W.isVersionAtLeast && W.isVersionAtLeast(version));

export const inTelegram = Boolean(W && W.initData);
export const initData = () => (W && W.initData) || '';
export const startParam = () => (W && W.initDataUnsafe && W.initDataUnsafe.start_param) || '';
export const tgUser = () => (W && W.initDataUnsafe && W.initDataUnsafe.user) || null;

function applyScheme() {
  document.documentElement.dataset.scheme = (W && W.colorScheme) || 'light';
}

export function init() {
  if (!W) return;
  W.ready();
  W.expand();
  applyScheme();
  W.onEvent('themeChanged', applyScheme);
  if (at('6.1')) {
    W.setHeaderColor('secondary_bg_color');
    W.setBackgroundColor('secondary_bg_color');
  }
  if (at('7.10')) W.setBottomBarColor('secondary_bg_color');
  // Lists scroll a lot; a vertical swipe must not close the app by accident.
  if (at('7.7')) W.disableVerticalSwipes();
}

// --- bottom buttons -------------------------------------------------------
// Native MainButton/SecondaryButton when available; otherwise the app renders
// an in-page fallback from `fallbackButtons()`.
const fallback = { main: null, secondary: null };
const fallbackListeners = new Set();
export const fallbackButtons = () => fallback;
export function onFallbackChange(fn) {
  fallbackListeners.add(fn);
  return () => fallbackListeners.delete(fn);
}

function bottomButton(kind, native) {
  let handler = null;
  return {
    set({ text, onClick, disabled = false, danger = false, progress = false }) {
      if (!native) {
        fallback[kind] = { text, onClick, disabled, danger };
        fallbackListeners.forEach((fn) => fn());
        return;
      }
      if (handler) native.offClick(handler);
      handler = () => {
        if (at('6.1')) W.HapticFeedback.impactOccurred('light');
        onClick && onClick();
      };
      const params = { text, is_active: !disabled, is_visible: true };
      const color = danger ? W.themeParams.destructive_text_color : W.themeParams.button_color;
      if (color) params.color = color;
      native.setParams(params);
      native.onClick(handler);
      if (progress) native.showProgress(false);
      else native.hideProgress();
    },
    hide() {
      if (!native) {
        fallback[kind] = null;
        fallbackListeners.forEach((fn) => fn());
        return;
      }
      if (handler) native.offClick(handler);
      handler = null;
      native.hide();
    },
  };
}

export const mainButton = bottomButton('main', W && W.initData ? W.MainButton : null);
export const secondaryButton = bottomButton('secondary', at('7.10') ? W.SecondaryButton : null);

function simpleButton(native, version) {
  let handler = null;
  const ok = native && at(version);
  return {
    show(fn) {
      if (!ok) return;
      if (handler) native.offClick(handler);
      handler = fn;
      native.onClick(fn);
      native.show();
    },
    hide() {
      if (!ok) return;
      if (handler) native.offClick(handler);
      handler = null;
      native.hide();
    },
  };
}

export const backButton = simpleButton(W && W.BackButton, '6.1');
export const settingsButton = simpleButton(W && W.SettingsButton, '7.0');

// --- feedback and dialogs -------------------------------------------------
export const haptic = {
  select: () => at('6.1') && W.HapticFeedback.selectionChanged(),
  tap: () => at('6.1') && W.HapticFeedback.impactOccurred('light'),
  ok: () => at('6.1') && W.HapticFeedback.notificationOccurred('success'),
  error: () => at('6.1') && W.HapticFeedback.notificationOccurred('error'),
  warn: () => at('6.1') && W.HapticFeedback.notificationOccurred('warning'),
};

/** Native confirm with a destructive button; resolves to true/false. */
export function confirm({ title, message, ok = 'OK', destructive = false }) {
  if (at('6.2')) {
    return new Promise((resolve) =>
      W.showPopup(
        {
          title,
          message,
          buttons: [
            { id: 'cancel', type: 'cancel' },
            { id: 'ok', type: destructive ? 'destructive' : 'default', text: ok },
          ],
        },
        (id) => resolve(id === 'ok'),
      ),
    );
  }
  return Promise.resolve(window.confirm(`${title}\n\n${message}`));
}

export function closingConfirmation(on) {
  if (!at('6.2')) return;
  if (on) W.enableClosingConfirmation();
  else W.disableClosingConfirmation();
}

const absolute = (url) => new URL(url, window.location.href).href;

/** Native download dialog (Bot API 8.0); older clients open the link. */
export function download(url, fileName) {
  const href = absolute(url);
  if (at('8.0')) {
    W.downloadFile({ url: href, file_name: fileName });
  } else {
    openLink(href);
  }
}

export function openLink(url) {
  const href = absolute(url);
  if (W && W.initData) W.openLink(href);
  else window.open(href, '_blank', 'noopener');
}

export function close() {
  if (W) W.close();
}
