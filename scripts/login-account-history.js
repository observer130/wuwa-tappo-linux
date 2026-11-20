(() => {
  // A second injection (another initialization path or window) must not add a
  // duplicate menu and a second set of document listeners.
  if (window.__wuwatappoLoginHistoryInstalled) return;
  window.__wuwatappoLoginHistoryInstalled = true;

  // WebView2 keeps a per-origin form history and suggests earlier values when
  // the official login form is focused. WebKitGTK has no such autofill, so the
  // login window remembers the most recent successfully used login names and
  // phone numbers and offers them under the matching field.
  //
  // Only the login name / phone number is stored; passwords never reach this
  // script. The SDK reports accepted logins through its public
  // `onLoginSuccessful` event, and values are persisted in the same
  // LocalStorage origin the official login state already uses.
  const ACCOUNT_FIELD = {
    id: 'anchor-account',
    key: 'wuwatappo-login-accounts',
    label: '最近登录的账号',
  };
  const PHONE_FIELD = {
    id: 'anchor-phone',
    key: 'wuwatappo-login-phones',
    label: '最近登录的手机号',
  };
  const FIELDS = [ACCOUNT_FIELD, PHONE_FIELD];
  const FIELD_BY_ID = new Map(FIELDS.map(field => [field.id, field]));
  const MAX_ENTRIES = 5;
  const MENU_ID = 'wuwatappo-login-history-menu';
  const STYLE_ID = 'wuwatappo-login-history-style';
  const OPTION_CLASS = 'wuwatappo-login-history-option';
  const STYLE = `
.${MENU_ID} {
  position: fixed;
  z-index: 2147483647;
  box-sizing: border-box;
  display: none;
  overflow-y: auto;
  max-width: calc(100vw - 16px);
  padding: 4px;
  border: 1px solid #d8dde7;
  border-radius: 8px;
  background: #fff;
  box-shadow: 0 10px 28px rgba(31, 38, 51, .18);
  color: #1f2633;
  font-family: inherit;
  font-size: 14px;
  line-height: 20px;
  user-select: none;
}
.${MENU_ID}[data-open="true"] { display: block; }
.${MENU_ID} .${OPTION_CLASS} {
  overflow: hidden;
  padding: 9px 10px;
  border-radius: 6px;
  cursor: pointer;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.${MENU_ID} .${OPTION_CLASS}[data-active="true"] { background: #eef3fb; }
`;

  const candidates = new Map();
  let boundEventBus = null;
  let menu = null;
  let menuInput = null;
  let menuEntries = [];
  let activeIndex = -1;
  let filling = false;
  let installed = false;
  let repositioning = false;

  function normalizeEntry(value) {
    return typeof value === 'string' ? value.trim().replace(/\s+/g, ' ').slice(0, 64) : '';
  }

  function fieldFor(target) {
    if (!target || typeof target !== 'object') return null;
    return FIELD_BY_ID.get(target.id) || null;
  }

  function readEntries(field) {
    try {
      const raw = window.localStorage.getItem(field.key);
      if (!raw) return [];
      const parsed = JSON.parse(raw);
      if (!Array.isArray(parsed)) return [];
      const seen = new Set();
      const entries = [];
      for (const value of parsed) {
        const entry = normalizeEntry(value);
        if (!entry) continue;
        const key = entry.toLowerCase();
        if (seen.has(key)) continue;
        seen.add(key);
        entries.push(entry);
        if (entries.length >= MAX_ENTRIES) break;
      }
      return entries;
    } catch {
      return [];
    }
  }

  function saveEntries(field, entries) {
    try {
      window.localStorage.setItem(field.key, JSON.stringify(entries.slice(0, MAX_ENTRIES)));
      return true;
    } catch {
      return false;
    }
  }

  function rememberEntry(field, value) {
    const entry = normalizeEntry(value);
    if (!entry) return false;
    const entries = readEntries(field).filter(item => item.toLowerCase() !== entry.toLowerCase());
    entries.unshift(entry);
    return saveEntries(field, entries);
  }

  function isMenuOpen() {
    return Boolean(menu && menu.getAttribute('data-open') === 'true' && menuEntries.length);
  }

  function clearInputAria(input) {
    if (!input) return;
    input.setAttribute('aria-expanded', 'false');
    input.removeAttribute('aria-activedescendant');
  }

  function closeMenu() {
    if (menu) {
      menu.setAttribute('data-open', 'false');
      menu.textContent = '';
    }
    clearInputAria(menuInput);
    menuInput = null;
    menuEntries = [];
    activeIndex = -1;
  }

  function ensureStyle() {
    if (document.getElementById(STYLE_ID)) return;
    const style = document.createElement('style');
    style.id = STYLE_ID;
    style.textContent = STYLE;
    (document.head || document.documentElement).appendChild(style);
  }

  function ensureMenu(field) {
    if (!menu) {
      const element = document.createElement('div');
      element.id = MENU_ID;
      element.className = MENU_ID;
      element.setAttribute('role', 'listbox');
      element.setAttribute('data-open', 'false');
      document.body.appendChild(element);
      menu = element;
    }
    menu.setAttribute('aria-label', field.label);
    return menu;
  }

  function positionMenu() {
    if (!menu || !menuInput || !isMenuOpen()) return;
    const anchor = menuInput.closest('div[class*="_input-wrapper_"]') || menuInput.parentElement || menuInput;
    const rect = anchor.getBoundingClientRect();
    const viewportWidth = window.innerWidth || document.documentElement.clientWidth || 0;
    const viewportHeight = window.innerHeight || document.documentElement.clientHeight || 0;
    if (viewportWidth <= 0 || viewportHeight <= 0) return;
    const width = Math.max(160, Math.min(rect.width || 0, viewportWidth - 16));
    const left = Math.max(8, Math.min(rect.left || 0, viewportWidth - width - 8));
    menu.style.width = `${width}px`;
    menu.style.left = `${left}px`;
    const spaceBelow = viewportHeight - (rect.bottom || 0) - 12;
    const spaceAbove = (rect.top || 0) - 12;
    if (spaceBelow < 120 && spaceAbove > spaceBelow) {
      menu.style.top = 'auto';
      menu.style.bottom = `${Math.max(8, viewportHeight - (rect.top || 0) + 4)}px`;
      menu.style.maxHeight = `${Math.max(48, Math.min(240, spaceAbove))}px`;
    } else {
      menu.style.top = `${(rect.bottom || 0) + 4}px`;
      menu.style.bottom = 'auto';
      menu.style.maxHeight = `${Math.max(48, Math.min(240, spaceBelow))}px`;
    }
  }

  function setActive(index) {
    if (!menu || !menuEntries.length) return;
    const length = menuEntries.length;
    activeIndex = ((index % length) + length) % length;
    const options = menu.children;
    for (let i = 0; i < options.length; i += 1) {
      const active = i === activeIndex;
      options[i].setAttribute('aria-selected', active ? 'true' : 'false');
      options[i].setAttribute('data-active', active ? 'true' : 'false');
    }
    if (menuInput) menuInput.setAttribute('aria-activedescendant', `${MENU_ID}-option-${activeIndex}`);
    const active = options[activeIndex];
    if (active && typeof active.scrollIntoView === 'function') active.scrollIntoView({ block: 'nearest' });
  }

  function renderMenu(input, field) {
    const entries = readEntries(field);
    const query = normalizeEntry(input.value).toLowerCase();
    const filtered = query ? entries.filter(entry => entry.toLowerCase().includes(query)) : entries;
    if (!filtered.length) {
      closeMenu();
      return false;
    }
    const element = ensureMenu(field);
    element.textContent = '';
    if (menuInput && menuInput !== input) clearInputAria(menuInput);
    menuInput = input;
    menuEntries = filtered;
    activeIndex = -1;
    // The rebuilt options restart at -1; a previous arrow-key selection must
    // not leave the input pointing at an option id that no longer exists.
    input.removeAttribute('aria-activedescendant');
    for (let i = 0; i < filtered.length; i += 1) {
      const option = document.createElement('div');
      option.className = OPTION_CLASS;
      option.id = `${MENU_ID}-option-${i}`;
      option.setAttribute('role', 'option');
      option.setAttribute('aria-selected', 'false');
      option.setAttribute('data-active', 'false');
      option.setAttribute('data-index', String(i));
      option.setAttribute('title', filtered[i]);
      option.textContent = filtered[i];
      element.appendChild(option);
    }
    input.setAttribute('aria-haspopup', 'listbox');
    input.setAttribute('aria-autocomplete', 'list');
    input.setAttribute('aria-expanded', 'true');
    input.setAttribute('aria-controls', MENU_ID);
    element.setAttribute('data-open', 'true');
    ensureStyle();
    positionMenu();
    return true;
  }

  function openMenu(input, field) {
    if (!input || !field || input.disabled) return false;
    bindSdkEvents();
    if (!readEntries(field).length) {
      closeMenu();
      return false;
    }
    return renderMenu(input, field);
  }

  function setInputValue(input, value) {
    filling = true;
    try {
      const inputPrototype = window.HTMLInputElement && window.HTMLInputElement.prototype;
      const setter = inputPrototype && Object.getOwnPropertyDescriptor(inputPrototype, 'value')?.set;
      if (setter) setter.call(input, value);
      else input.value = value;
      input.dispatchEvent(new Event('input', { bubbles: true }));
    } finally {
      filling = false;
    }
    const field = fieldFor(input);
    if (field) candidates.set(field.id, normalizeEntry(value));
    try {
      input.focus({ preventScroll: true });
    } catch {
      input.focus();
    }
    if (typeof input.setSelectionRange === 'function') input.setSelectionRange(value.length, value.length);
  }

  function choose(index) {
    const value = menuEntries[index];
    const input = menuInput;
    if (!value || !input) return;
    closeMenu();
    setInputValue(input, value);
  }

  function handleFocusIn(event) {
    const field = fieldFor(event.target);
    if (!field) {
      closeMenu();
      return;
    }
    openMenu(event.target, field);
  }

  function handleFocusOut(event) {
    if (!fieldFor(event.target) || !isMenuOpen()) return;
    const next = event.relatedTarget;
    if (!next || !menu || !menu.contains(next)) closeMenu();
  }

  // The SDK sanitizes the value in its own bubbling input handler, which runs
  // after this capture listener. Remember the settled value on the next
  // microtask so history never keeps a raw paste such as "+86 138 0013 8000".
  function rememberSettledValue(input, field) {
    Promise.resolve().then(() => {
      if (fieldFor(input) !== field) return;
      candidates.set(field.id, normalizeEntry(input.value));
    });
  }

  function handleInput(event) {
    const field = fieldFor(event.target);
    if (!field) return;
    const input = event.target;
    candidates.set(field.id, normalizeEntry(input.value));
    bindSdkEvents();
    if (filling) return;
    openMenu(input, field);
    rememberSettledValue(input, field);
  }

  function handleKeyDown(event) {
    const field = fieldFor(event.target);
    if (!field) return;
    bindSdkEvents();
    if (!event.key || event.isComposing || event.keyCode === 229) return;
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      if (!isMenuOpen() && !openMenu(event.target, field)) return;
      event.preventDefault();
      event.stopPropagation();
      if (activeIndex < 0) setActive(event.key === 'ArrowDown' ? 0 : menuEntries.length - 1);
      else setActive(activeIndex + (event.key === 'ArrowDown' ? 1 : -1));
      return;
    }
    if (!isMenuOpen()) return;
    if (event.key === 'Enter') {
      if (activeIndex < 0) return;
      event.preventDefault();
      event.stopPropagation();
      choose(activeIndex);
      return;
    }
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      closeMenu();
      return;
    }
    if (event.key === 'Tab') closeMenu();
  }

  function handlePointerDown(event) {
    if (!isMenuOpen()) return;
    const target = event.target;
    if (menu && menu.contains(target)) {
      event.preventDefault();
      const option = target && typeof target.closest === 'function'
        ? target.closest(`.${OPTION_CLASS}`)
        : null;
      if (option && menu.contains(option)) {
        event.stopPropagation();
        const index = Number(option.getAttribute('data-index'));
        if (Number.isInteger(index) && index >= 0) choose(index);
      }
      return;
    }
    if (!fieldFor(target)) closeMenu();
  }

  function handleClick(event) {
    const field = fieldFor(event.target);
    if (field) openMenu(event.target, field);
  }

  function reposition() {
    if (repositioning) return;
    repositioning = true;
    try {
      positionMenu();
    } finally {
      repositioning = false;
    }
  }

  function bindSdkEvents() {
    const sdk = window.KrSDK;
    const bus = sdk && sdk.event;
    if (!bus || typeof bus.on !== 'function' || boundEventBus === bus) return;
    boundEventBus = bus;
    try {
      bus.on('onLoginSuccessful', payload => {
        if (!payload || typeof payload !== 'object') return;
        const phone = normalizeEntry(payload.phone);
        if (phone) {
          // Phone/code login. The account form never sets `phone`, which keeps
          // the two histories apart even when the same digits are valid there.
          const value = candidates.get(PHONE_FIELD.id);
          if (value) {
            rememberEntry(PHONE_FIELD, value);
            candidates.set(PHONE_FIELD.id, '');
          }
          return;
        }
        // The echoed username is the canonical account name, which differs
        // from what was typed whenever the identifier is an email or phone
        // number (or when the server trims/normalizes case). A successful
        // login means the value currently in the account field is the one that
        // worked, so remember that instead of requiring a byte-identical echo.
        const username = normalizeEntry(payload.username);
        const account = candidates.get(ACCOUNT_FIELD.id);
        if (username && account) {
          rememberEntry(ACCOUNT_FIELD, account);
        }
        candidates.set(ACCOUNT_FIELD.id, '');
      });
    } catch {
      boundEventBus = null;
    }
  }

  function install() {
    if (installed) return;
    installed = true;
    document.addEventListener('focusin', handleFocusIn, true);
    document.addEventListener('focusout', handleFocusOut, true);
    document.addEventListener('input', handleInput, true);
    document.addEventListener('keydown', handleKeyDown, true);
    document.addEventListener('pointerdown', handlePointerDown, true);
    document.addEventListener('click', handleClick, true);
    window.addEventListener('resize', reposition, { passive: true });
    window.addEventListener('scroll', reposition, { passive: true, capture: true });
    window.addEventListener('blur', closeMenu);
    window.addEventListener('pagehide', closeMenu);
    document.addEventListener('visibilitychange', () => {
      if (document.visibilityState === 'hidden') closeMenu();
    });
    bindSdkEvents();
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', install, { once: true });
  else install();
})();
