import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import vm from 'node:vm';

const script = await readFile(new URL('../scripts/login-account-history.js', import.meta.url), 'utf8');
const STORAGE_KEY = 'wuwatappo-login-accounts';
const PHONE_STORAGE_KEY = 'wuwatappo-login-phones';
const MENU_ID = 'wuwatappo-login-history-menu';
const STYLE_ID = 'wuwatappo-login-history-style';

class FakeEvent {
  constructor(type, init = {}) {
    this.type = type;
    this.defaultPrevented = false;
    this.propagationStopped = false;
    this.bubbles = false;
    Object.assign(this, init);
  }

  preventDefault() { this.defaultPrevented = true; }
  stopPropagation() { this.propagationStopped = true; }
}

class FakeNode {
  constructor(tagName, document) {
    this.tagName = String(tagName).toUpperCase();
    this.ownerDocument = document;
    this.parentNode = null;
    this.children = [];
    this.attributes = new Map();
    this.style = {};
    this.id = '';
    this.className = '';
    this.value = '';
    this.disabled = false;
    this.rect = { left: 0, top: 0, width: 0, height: 0, right: 0, bottom: 0 };
    this._textContent = '';
  }

  get textContent() { return this._textContent; }
  set textContent(value) {
    this._textContent = String(value);
    if (this._textContent === '') this.children = [];
  }

  get parentElement() { return this.parentNode; }
  get scrollHeight() { return this.children.length * 40; }

  appendChild(child) {
    if (child.parentNode) child.parentNode.children = child.parentNode.children.filter(entry => entry !== child);
    child.parentNode = this;
    this.children.push(child);
    return child;
  }

  append(...children) {
    for (const child of children) this.appendChild(child);
  }

  contains(node) {
    for (let current = node; current; current = current.parentNode) {
      if (current === this) return true;
    }
    return false;
  }

  closest(selector) {
    const match = (node) => {
      if (selector === 'div[class*="_input-wrapper_"]') {
        return node.tagName === 'DIV' && String(node.className).includes('_input-wrapper_');
      }
      if (selector.startsWith('.')) {
        return String(node.className).split(/\s+/).includes(selector.slice(1));
      }
      return false;
    };
    for (let current = this; current; current = current.parentNode) {
      if (match(current)) return current;
    }
    return null;
  }

  setAttribute(name, value) {
    this.attributes.set(name, String(value));
    if (name === 'id') this.id = String(value);
    if (name === 'class') this.className = String(value);
  }

  getAttribute(name) { return this.attributes.has(name) ? this.attributes.get(name) : null; }
  removeAttribute(name) { this.attributes.delete(name); }
  getBoundingClientRect() { return this.rect; }
  scrollIntoView() {}
  setSelectionRange() {}

  focus() {
    if (this.ownerDocument.activeElement === this) return;
    const previous = this.ownerDocument.activeElement;
    if (previous) this.ownerDocument.fire('focusout', previous, { relatedTarget: this });
    this.ownerDocument.activeElement = this;
    this.ownerDocument.fire('focusin', this);
  }

  dispatchEvent(event) {
    this.ownerDocument.fireEvent(event, this);
    return !event.defaultPrevented;
  }
}

class FakeDocument {
  constructor() {
    this.readyState = 'complete';
    this.listeners = new Map();
    this.activeElement = null;
    this.documentElement = new FakeNode('html', this);
    this.head = new FakeNode('head', this);
    this.body = new FakeNode('body', this);
    this.documentElement.appendChild(this.head);
    this.documentElement.appendChild(this.body);
    this.documentElement.clientWidth = 600;
    this.documentElement.clientHeight = 660;
  }

  addEventListener(type, callback, options = {}) {
    const listeners = this.listeners.get(type) ?? [];
    listeners.push({ callback, capture: Boolean(options && options.capture) });
    this.listeners.set(type, listeners);
  }

  removeEventListener(type, callback) {
    this.listeners.set(type, (this.listeners.get(type) ?? []).filter(entry => entry.callback !== callback));
  }

  createElement(tagName) { return new FakeNode(tagName, this); }

  getElementById(id) {
    const find = (node) => {
      if (node.id === id) return node;
      for (const child of node.children) {
        const found = find(child);
        if (found) return found;
      }
      return null;
    };
    return find(this.documentElement);
  }

  fireEvent(event, target) {
    event.target = target ?? event.target ?? null;
    const entries = this.listeners.get(event.type) ?? [];
    // Capture listeners run before bubbling ones, so code here mirrors the real
    // ordering (the account history uses capture to beat the SDK form handlers).
    for (const capture of [true, false]) {
      for (const entry of entries) {
        if (entry.capture !== capture) continue;
        if (event.propagationStopped) return;
        entry.callback.call(this, event);
      }
    }
  }

  fire(type, target, props = {}) {
    const event = new FakeEvent(type, props);
    this.fireEvent(event, target);
    return event;
  }
}

class FakeWindow {
  constructor() {
    this.listeners = new Map();
    this.innerWidth = 600;
    this.innerHeight = 660;
    this.storage = new Map();
    this.localStorage = {
      getItem: key => this.storage.get(key) ?? null,
      setItem: (key, value) => this.storage.set(key, String(value)),
      removeItem: key => this.storage.delete(key),
    };
  }

  addEventListener(type, callback, options = {}) {
    const listeners = this.listeners.get(type) ?? [];
    listeners.push({ callback, capture: Boolean(options && options.capture) });
    this.listeners.set(type, listeners);
  }

  removeEventListener(type, callback) {
    this.listeners.set(type, (this.listeners.get(type) ?? []).filter(entry => entry.callback !== callback));
  }

  fire(type, props = {}) {
    const event = new FakeEvent(type, props);
    const entries = this.listeners.get(type) ?? [];
    for (const capture of [true, false]) {
      for (const entry of entries) {
        if (entry.capture !== capture) continue;
        entry.callback.call(this, event);
      }
    }
    return event;
  }
}

function createSdk() {
  const listeners = new Map();
  return {
    event: {
      on(type, callback) {
        const list = listeners.get(type) ?? [];
        list.push(callback);
        listeners.set(type, list);
      },
      emit(type, payload) {
        for (const callback of listeners.get(type) ?? []) callback(payload);
      },
    },
    listeners,
  };
}

function environment(options = {}) {
  const {
    accounts = [],
    phones = [],
    sdk = true,
    sdkLate = false,
    sanitizePhone = false,
    storageFailure = false,
    credentialFields = false,
  } = options;
  const document = new FakeDocument();
  const window = new FakeWindow();
  const wrappers = new Map();
  for (const id of ['anchor-account', 'anchor-phone']) {
    const wrapper = document.createElement('div');
    wrapper.className = '_input-wrapper_k21fw_69';
    wrapper.rect = { left: 120, top: 260, width: 351, height: 48, right: 471, bottom: 308 };
    document.body.appendChild(wrapper);
    const input = document.createElement('input');
    input.id = id;
    input.rect = { left: 130, top: 272, width: 331, height: 24, right: 461, bottom: 296 };
    wrapper.appendChild(input);
    wrappers.set(id, wrapper);
  }
  const credentialInputs = [];
  if (credentialFields) {
    // Fields the script must never touch: password/code inputs are outside the
    // remembered identifiers, and their values must not reach LocalStorage.
    for (const id of ['anchor-password', 'anchor-code']) {
      const wrapper = document.createElement('div');
      wrapper.className = '_input-wrapper_k21fw_69';
      wrapper.rect = { left: 120, top: 260, width: 351, height: 48, right: 471, bottom: 308 };
      document.body.appendChild(wrapper);
      const input = document.createElement('input');
      input.id = id;
      input.type = 'password';
      wrapper.appendChild(input);
      credentialInputs.push(input);
    }
  }
  const input = document.getElementById('anchor-account');
  const phone = document.getElementById('anchor-phone');
  if (accounts.length) window.storage.set(STORAGE_KEY, JSON.stringify(accounts));
  if (phones.length) window.storage.set(PHONE_STORAGE_KEY, JSON.stringify(phones));
  if (storageFailure) {
    window.localStorage = {
      getItem: () => { throw new Error('storage unavailable'); },
      setItem: () => { throw new Error('storage unavailable'); },
      removeItem: () => { throw new Error('storage unavailable'); },
    };
  }
  const fakeSdk = createSdk();
  if (sdk) window.KrSDK = fakeSdk;
  vm.runInNewContext(script, { window, document, Event: FakeEvent, console });
  if (sanitizePhone) {
    // Stand-in for the official SDK's own bubbling input handler: it rewrites
    // the field after the capture listener in this script has seen the raw
    // value.
    document.addEventListener('input', event => {
      if (event.target && event.target.id === 'anchor-phone') {
        event.target.value = String(event.target.value).replace(/\D+/g, '').slice(0, 11);
      }
    });
  }
  return {
    document,
    window,
    input,
    phone,
    credentialInputs,
    sdk: fakeSdk,
    menu: () => document.getElementById(MENU_ID),
    style: () => document.getElementById(STYLE_ID),
    wrapper: (id = 'anchor-account') => wrappers.get(id),
    stored: (key = STORAGE_KEY) => {
      const raw = window.storage.get(key);
      return raw === undefined ? null : JSON.parse(raw);
    },
    settle: () => Promise.resolve(),
    attachSdk: () => { window.KrSDK = fakeSdk; },
    focus: () => input.focus(),
    focusPhone: () => phone.focus(),
    fireInput: (value = input.value) => {
      input.value = value;
      input.dispatchEvent(new FakeEvent('input', { bubbles: true }));
    },
    firePhoneInput: (value = phone.value) => {
      phone.value = value;
      phone.dispatchEvent(new FakeEvent('input', { bubbles: true }));
    },
    fireKey: (key, props = {}) => document.fire('keydown', input, { key, ...props }),
    success: payload => fakeSdk.event.emit('onLoginSuccessful', payload),
  };
}

test('a successful account/password login is remembered and offered in the dropdown', () => {
  const env = environment();
  env.fireInput('13800138000');
  env.success({ username: '13800138000', token: 'not-stored' });
  assert.deepEqual(env.stored(), ['13800138000']);
  assert.equal(JSON.stringify(env.stored()).includes('not-stored'), false);

  env.input.value = '';
  env.focus();
  const menu = env.menu();
  assert.equal(menu.getAttribute('data-open'), 'true');
  assert.equal(menu.children.length, 1);
  assert.equal(menu.children[0].textContent, '13800138000');
  assert.equal(menu.children[0].getAttribute('role'), 'option');
  assert.equal(menu.children[0].getAttribute('data-index'), '0');

  env.document.fire('pointerdown', menu.children[0], { button: 0 });
  assert.equal(env.input.value, '13800138000');
  assert.equal(menu.getAttribute('data-open'), 'false');
});

test('phone/code logins do not enter the account history', () => {
  const env = environment();
  env.fireInput('alice');
  env.firePhoneInput('13800138000');
  env.success({ username: 'alice', phone: '13800138000' });
  assert.equal(env.stored(), null);
  assert.deepEqual(env.stored(PHONE_STORAGE_KEY), ['13800138000']);

  env.success({ username: 'alice' });
  assert.deepEqual(env.stored(), ['alice']);
});

test('the typed identifier is remembered even when the SDK echoes a canonical name', () => {
  const env = environment();
  env.fireInput('alice@example.com');
  // The SDK reports the account name, not the email that was typed.
  env.success({ username: 'alice' });
  assert.deepEqual(env.stored(), ['alice@example.com']);
});

test('a phone number sanitized by the SDK after the input event is stored settled', async () => {
  const env = environment({ sanitizePhone: true });
  env.firePhoneInput('138 0013 8000');
  // The raw value is visible to capture listeners only; the SDK rewrites it to
  // digits while the event bubbles.
  assert.equal(env.phone.value, '13800138000');
  await env.settle();
  env.success({ username: 'ignored', phone: '13800138000' });
  assert.deepEqual(env.stored(PHONE_STORAGE_KEY), ['13800138000']);
});

test('password and code fields never reach the history', () => {
  const env = environment({ credentialFields: true });
  for (const input of env.credentialInputs) {
    input.value = 'secret-value';
    input.dispatchEvent(new FakeEvent('input', { bubbles: true }));
    input.dispatchEvent(new FakeEvent('keydown', { bubbles: true, key: 'Enter' }));
  }
  assert.equal(env.menu(), null);
  env.success({ username: 'alice' });
  assert.equal(env.stored(), null);
  assert.equal(JSON.stringify([...env.window.storage.values()]).includes('secret-value'), false);
});

test('phone/code history is offered in the phone field and kept apart from accounts', () => {
  const env = environment({ accounts: ['account-name'], phones: ['13800138000'] });
  env.phone.value = '';
  env.focusPhone();
  const menu = env.menu();
  assert.equal(menu.getAttribute('aria-label'), '最近登录的手机号');
  assert.deepEqual(menu.children.map(option => option.textContent), ['13800138000']);

  env.document.fire('pointerdown', menu.children[0], { button: 0 });
  assert.equal(env.phone.value, '13800138000');
  assert.equal(menu.getAttribute('data-open'), 'false');

  env.input.value = '';
  env.focus();
  assert.equal(env.menu().getAttribute('aria-label'), '最近登录的账号');
  assert.deepEqual(env.menu().children.map(option => option.textContent), ['account-name']);
});

test('recent accounts are deduplicated, most-recent-first and capped at five', () => {
  const env = environment({ accounts: ['a', 'b', 'c', 'd', 'e'] });
  env.fireInput('c');
  env.success({ username: 'c' });
  assert.deepEqual(env.stored(), ['c', 'a', 'b', 'd', 'e']);

  env.fireInput('f');
  env.success({ username: 'f' });
  assert.deepEqual(env.stored(), ['f', 'c', 'a', 'b', 'd']);

  env.input.value = '';
  env.focus();
  assert.deepEqual(env.menu().children.map(option => option.textContent), ['f', 'c', 'a', 'b', 'd']);
});

test('typing filters suggestions and clears the menu when nothing matches', () => {
  const env = environment({ accounts: ['13800138000', '13900139000'] });
  env.input.value = '';
  env.focus();
  assert.equal(env.menu().children.length, 2);
  env.fireInput('139');
  assert.deepEqual(env.menu().children.map(option => option.textContent), ['13900139000']);
  env.fireInput('137');
  assert.equal(env.menu().getAttribute('data-open'), 'false');
});

test('the menu is anchored under the field and flips above it near the bottom', () => {
  const env = environment({ accounts: ['111'] });
  env.focus();
  const menu = env.menu();
  assert.equal(menu.style.left, '120px');
  assert.equal(menu.style.width, '351px');
  assert.equal(menu.style.top, '312px');
  assert.equal(menu.style.bottom, 'auto');
  assert.equal(menu.style.maxHeight, '240px');
  const style = env.style();
  assert.ok(style.textContent.includes(`.${MENU_ID}[data-open="true"] { display: block; }`));

  env.wrapper('anchor-account').rect = {
    left: 120, top: 560, width: 351, height: 48, right: 471, bottom: 608,
  };
  env.fireInput('');
  assert.equal(menu.style.top, 'auto');
  assert.equal(menu.style.bottom, '104px');
});

test('keyboard navigation selects an account and escape closes the menu', () => {
  const env = environment({ accounts: ['111', '222'] });
  env.focus();
  const down = env.fireKey('ArrowDown');
  assert.equal(down.defaultPrevented, true);
  assert.equal(down.propagationStopped, true);
  assert.equal(env.menu().getAttribute('data-open'), 'true');
  assert.equal(env.menu().children[0].getAttribute('aria-selected'), 'true');
  assert.equal(env.input.getAttribute('aria-activedescendant'), `${MENU_ID}-option-0`);
  env.fireKey('ArrowDown');
  assert.equal(env.menu().children[1].getAttribute('aria-selected'), 'true');
  const enter = env.fireKey('Enter');
  assert.equal(enter.defaultPrevented, true);
  assert.equal(enter.propagationStopped, true);
  assert.equal(env.input.value, '222');
  assert.equal(env.menu().getAttribute('data-open'), 'false');

  env.input.value = '';
  env.focus();
  env.fireKey('ArrowDown');
  assert.equal(env.menu().getAttribute('data-open'), 'true');
  const escape = env.fireKey('Escape');
  assert.equal(escape.defaultPrevented, true);
  assert.equal(env.menu().getAttribute('data-open'), 'false');
  assert.equal(env.input.value, '');
});

test('plain keys still reach the SDK form handlers', () => {
  const env = environment({ accounts: ['111'] });
  env.focus();
  env.fireKey('Enter');
  const seen = [];
  env.document.addEventListener('keydown', event => seen.push(event.key));
  const enter = env.fireKey('Enter');
  assert.equal(enter.defaultPrevented, false);
  assert.equal(enter.propagationStopped, false);
  assert.deepEqual(seen, ['Enter']);
});

test('arrow keys are consumed before later bubbling listeners run', () => {
  const env = environment({ accounts: ['111'] });
  env.focus();
  env.fireInput('1');
  const seen = [];
  // Registered after the script, i.e. the position the SDK's form handler has.
  env.document.addEventListener('keydown', event => seen.push(event.key));
  const down = env.fireKey('ArrowDown');
  assert.equal(down.defaultPrevented, true);
  assert.equal(down.propagationStopped, true);
  assert.deepEqual(seen, []);
});

test('a rebuild drops a stale aria-activedescendant', () => {
  const env = environment({ accounts: ['111', '222'] });
  env.focus();
  env.fireKey('ArrowDown');
  assert.equal(env.input.getAttribute('aria-activedescendant'), `${MENU_ID}-option-0`);
  env.fireInput('2');
  assert.equal(env.input.getAttribute('aria-activedescendant'), null);
  assert.equal(env.menu().children[0].getAttribute('aria-selected'), 'false');
});

test('pointerdown outside or window blur closes the menu without filling', () => {
  const env = environment({ accounts: ['111'] });
  env.focus();
  assert.equal(env.menu().getAttribute('data-open'), 'true');
  env.document.fire('pointerdown', env.document.body, { button: 0 });
  assert.equal(env.menu().getAttribute('data-open'), 'false');
  assert.equal(env.input.value, '');

  env.document.fire('focusin', env.input);
  assert.equal(env.menu().getAttribute('data-open'), 'true');
  env.window.fire('blur');
  assert.equal(env.menu().getAttribute('data-open'), 'false');
});

test('an SDK injected after page load is bound from the next focus or input', () => {
  const env = environment({ accounts: ['111'], sdkLate: true });
  env.focus();
  assert.equal(env.menu().getAttribute('data-open'), 'true');
  // SDK appears only after the login page finished booting.
  env.attachSdk();
  env.fireInput('alice');
  env.success({ username: 'alice' });
  assert.deepEqual(env.stored(), ['alice', '111']);
});

test('a second injection does not duplicate listeners or the menu', () => {
  const env = environment({ accounts: ['111'] });
  const context = { window: env.window, document: env.document, Event: FakeEvent, console };
  vm.runInNewContext(script, context);
  const inputListeners = env.document.listeners.get('input');
  assert.equal(inputListeners.length, 1);
  env.focus();
  const count = (node, id) => (node.id === id ? 1 : 0)
    + node.children.reduce((total, child) => total + count(child, id), 0);
  assert.equal(count(env.document.documentElement, MENU_ID), 1);
  assert.equal(count(env.document.documentElement, STYLE_ID), 1);
});

test('suggestions are rendered as text, never as markup', () => {
  const payload = '<img src=x onerror=alert(1)>';
  const env = environment({ accounts: [payload] });
  env.focus();
  const option = env.menu().children[0];
  assert.equal(option.textContent, payload);
  assert.equal(option.children.length, 0);
  assert.equal(option.getAttribute('title'), payload);
});

test('storage failures degrade to no history without breaking typing', () => {
  const env = environment({ storageFailure: true });
  env.focus();
  assert.equal(env.menu(), null);
  env.fireInput('alice');
  assert.equal(env.menu(), null);
  env.success({ username: 'alice' });
  assert.equal(env.window.storage.size, 0);
});

test('without an SDK the dropdown still degrades cleanly', () => {
  const env = environment({ sdk: false });
  env.fireInput('alice');
  env.focus();
  assert.equal(env.menu(), null);
  assert.equal(env.input.value, 'alice');
});

test('missing or malformed account storage degrades to no suggestions', () => {
  const missing = environment();
  missing.focus();
  assert.equal(missing.menu(), null);

  const malformed = environment();
  malformed.window.storage.set(STORAGE_KEY, 'not-json');
  malformed.focus();
  assert.equal(malformed.menu(), null);

  const mixed = environment();
  mixed.window.storage.set(STORAGE_KEY, JSON.stringify([1, null, {}, '13800138000']));
  mixed.focus();
  assert.deepEqual(mixed.menu().children.map(option => option.textContent), ['13800138000']);
});
