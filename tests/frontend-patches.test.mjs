import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {
  patchAbout,
  patchChestTimers,
  patchCookieStorage,
  patchDomesticSdkUrls,
  patchIndexLanguage,
  patchLoginReady,
  patchPetCursor,
  patchRelaunch,
  patchSessionRestore,
  PET_CURSOR_ANCHORS,
  PET_CURSOR_MARKER,
  RELAUNCH_SOURCE,
  SESSION_RESTORE_SOURCE,
} from '../scripts/frontend-patches.mjs';

test('CN SDK script and stylesheet use HTTPS from the local Tauri origin', () => {
  const source = 'const APP_REGION="CN";const isdkSrc="//pro-cdn-sdk.kurogame.com/pro/web-sdk/1.x/kr-sdk.js",isdkCss="//pro-cdn-sdk.kurogame.com/pro/web-sdk/1.x/style.css";';
  const result = patchDomesticSdkUrls(source);
  assert.ok(result.includes('isdkSrc="https://pro-cdn-sdk.kurogame.com/pro/web-sdk/1.x/kr-sdk.js"'));
  assert.ok(result.includes('isdkCss="https://pro-cdn-sdk.kurogame.com/pro/web-sdk/1.x/style.css"'));
  assert.throws(() => patchDomesticSdkUrls(source.replace('"CN"', '"GLOBAL"')), /official CN frontend/);
  assert.throws(() => patchDomesticSdkUrls('const APP_REGION="CN";'), /Expected 2 CN SDK URLs/);
});

const COOKIE_HELPERS = 'getCookie=e=>{const t=`kuro_${e}=`,n=document.cookie.split(";");for(let s=0;s<n.length;s+=1){let o=n[s];for(;o.charAt(0)===" ";)o=o.substring(1,o.length);if(o.indexOf(t)===0)return o.substring(t.length,o.length)}return null},setCookie=(e,t,n)=>{const s=new Date(Date.now()+n*24*60*60*1e3).toUTCString();document.cookie=`kuro_${e}=${t}; expires=${s}; path=/; SameSite=None; Secure`}';

test('login tokens survive when WebKitGTK rejects tauri:// cookies', () => {
  const store = new Map();
  const context = {
    document: {
      get cookie() { throw new Error('tauri:// cookies are unavailable'); },
      set cookie(_value) { throw new Error('tauri:// cookies are unavailable'); },
    },
    localStorage: {
      getItem: key => store.get(key) ?? null,
      setItem: (key, value) => store.set(key, String(value)),
      removeItem: key => store.delete(key),
    },
    Date, JSON, Number, String,
  };
  vm.runInNewContext(
    `${patchCookieStorage(COOKIE_HELPERS)};globalThis.readCookie=getCookie;globalThis.writeCookie=setCookie;`,
    context,
  );
  context.writeCookie('infoToken', 'token-value', 7);
  assert.equal(context.readCookie('infoToken'), 'token-value');
  assert.ok(store.has('wuwatappo-cookie-infoToken'));

  store.set('wuwatappo-cookie-infoToken', JSON.stringify({ value: 'expired', expires: Date.now() - 1 }));
  assert.equal(context.readCookie('infoToken'), null);
  assert.equal(store.has('wuwatappo-cookie-infoToken'), false);

  context.writeCookie('infoToken', 'token-value', 7);
  context.writeCookie('infoToken', '', -1);
  assert.equal(context.readCookie('infoToken'), null);
  assert.equal(store.has('wuwatappo-cookie-infoToken'), false);
  assert.throws(() => patchCookieStorage('getCookie=e=>{}'), /cookie helpers changed/);
});

const CHEST_SOURCE = 'function oo(){if(!A.isLoggedIn){Yt.value=0,Kt.value=0,Xo.value=0,zo.value=0;return}if(!St.value)return;const e=ys(),t=performance.now();Yt.value=xr===null?Co(A.userInfo?.left_treasure_chest,e):Math.max(0,xr-t),Kt.value=Or===null?Co(A.userInfo?.right_treasure_chest,e):Math.max(0,Or-t),ri("left",Yt.value),ri("right",Kt.value)}function ai(e){return qa.value?e||Tr.value?!0:bt.value:!1}const si=pe(()=>ii.value==="right"?!1:ii.value==="left"?ai(!0):ai(ro.value)),li=pe(()=>ii.value==="left"?!1:(ii.value==="right",ai(io.value)));';

test('chest timers are not blocked by activity config readiness', () => {
  const patched = patchChestTimers(CHEST_SOURCE);
  assert.ok(patched.includes('St.value=!0;'));
  assert.ok(!patched.includes('if(!St.value)return;'));
  assert.ok(patched.includes('!!((A.isLoggedIn&&Tr.value)||qa.value&&(e||bt.value))'));
  assert.ok(patched.includes('const si=pe(()=>A.isLoggedIn&&Tr.value?!0:'));
  assert.throws(() => patchChestTimers('function oo(){if(!St.value)return;}'), /Chest timer or visibility/);

  const ref = value => ({ value });
  const timerState = {
    A: { isLoggedIn: true, userInfo: { left_treasure_chest: 111, right_treasure_chest: 222 } },
    St: ref(false),
    Yt: ref(0),
    Kt: ref(0),
    Xo: ref(0),
    zo: ref(0),
    xr: null,
    Or: null,
    ys: () => 1000,
    performance: { now: () => 500 },
    Co: value => value,
    ri: () => {},
  };
  const timerSnippet = patched.slice(patched.indexOf('function oo('), patched.indexOf('function ai('));
  vm.runInNewContext(`${timerSnippet};globalThis.runTimer=oo;`, timerState);
  timerState.runTimer();
  assert.equal(timerState.St.value, true, 'readiness is marked on first computation');
  assert.equal(timerState.Yt.value, 111, 'left timer falls back to local computation');
  assert.equal(timerState.Kt.value, 222, 'right timer falls back to local computation');

  timerState.A.isLoggedIn = false;
  timerState.Yt.value = 9;
  timerState.Kt.value = 9;
  timerState.runTimer();
  assert.equal(timerState.Yt.value, 0);
  assert.equal(timerState.Kt.value, 0);
});

test('chest panes follow official visibility rules with port always-show', () => {
  const patched = patchChestTimers(CHEST_SOURCE);
  const snippet = patched.slice(patched.indexOf('function ai(e)'));
  const ref = value => ({ value });
  const state = {
    A: { isLoggedIn: true },
    qa: ref(true),
    Tr: ref(false),
    bt: ref(false),
    ro: ref(false),
    io: ref(false),
    ii: ref(null),
    pe: compute => ({ get value() { return compute(); } }),
  };
  vm.runInNewContext(`${snippet};globalThis.panes=()=>[si.value,li.value];`, state);
  const panesWith = overrides => {
    const values = {
      isLoggedIn: true, qa: true, Tr: false, bt: false, ro: false, io: false, ii: null,
      ...overrides,
    };
    state.A.isLoggedIn = values.isLoggedIn;
    state.qa.value = values.qa;
    state.Tr.value = values.Tr;
    state.bt.value = values.bt;
    state.ro.value = values.ro;
    state.io.value = values.io;
    state.ii.value = values.ii;
    return [...state.panes()];
  };

  // Official: hovering the pet reveals the countdown panes.
  assert.deepEqual(panesWith({ bt: true }), [true, true]);
  assert.deepEqual(panesWith({}), [false, false]);
  // A ready chest stays visible without hover, on either side.
  assert.deepEqual(panesWith({ ro: true }), [true, false]);
  assert.deepEqual(panesWith({ io: true }), [false, true]);
  assert.deepEqual(panesWith({ ro: true, io: true }), [true, true]);
  // The balance gate hides panes even when hovering or a chest is ready.
  assert.deepEqual(panesWith({ qa: false, bt: true }), [false, false]);
  assert.deepEqual(panesWith({ qa: false, ro: true, io: true }), [false, false]);
  // "Always show chest" bypasses the balance gate when logged in.
  assert.deepEqual(panesWith({ Tr: true, qa: false }), [true, true]);
  // Below 2000 clicks the official view shows only the affordable side.
  assert.deepEqual(panesWith({ ii: 'right', ro: false, io: true }), [false, true]);
  assert.deepEqual(panesWith({ ii: 'left', ro: true, io: false }), [true, false]);
  // Logged-out sessions keep the official two-button login prompt.
  assert.deepEqual(panesWith({ isLoggedIn: false, ro: true, io: true }), [true, true]);
  assert.deepEqual(panesWith({ isLoggedIn: false, Tr: true, ro: true, io: true }), [true, true]);
});

test('relaunch goes through the native restart command', () => {
  const patched = patchRelaunch(RELAUNCH_SOURCE);
  assert.ok(patched.includes('invoke("restart_app")'));
  assert.ok(!patched.includes('plugin:process|restart'));
  assert.throws(() => patchRelaunch('async function relaunch(){}'), /relaunch changed/);
});

test('login readiness is reported after the Vue listener is installed', () => {
  const result = patchLoginReady('function N(n){return o(u,t=>{n(t.payload??{})})}');
  assert.ok(result.indexOf('await o(u,') < result.indexOf('invoke("login_window_ready")'));
  assert.ok(result.includes('return unlisten'));
  assert.throws(() => patchLoginReady(''), /listener bridge changed/);
});

test('session restore retries transport failures but not auth rejections', async () => {
  const patched = patchSessionRestore(SESSION_RESTORE_SOURCE);
  assert.ok(patched.includes('for(let t=0;t<3;t+=1)'));
  assert.ok(patched.includes('s==="network_error"||s==="timeout"'));
  assert.throws(() => patchSessionRestore('function an(e={}){}'), /session restore flow changed/);

  async function run(failures) {
    const attempts = [];
    const context = {
      I: null,
      X: () => {},
      ce: () => 'token',
      ve: async () => {
        attempts.push(1);
        const failure = failures[attempts.length - 1];
        if (failure) throw { reason: failure };
      },
      window: { dispatchEvent: () => {} },
      CustomEvent: class {},
      console: { error: () => {} },
      l: () => {},
      o: () => {},
      S: error => error.reason,
      setTimeout: callback => callback(),
      Promise,
      Error,
    };
    vm.runInNewContext(`${patched};globalThis.bootstrap=sn;`, context);
    const restored = await context.bootstrap({ emitLoginSuccess: false });
    return { restored, attempts: attempts.length };
  }

  assert.deepEqual(await run(['network_error', 'network_error']), { restored: true, attempts: 3 });
  assert.deepEqual(await run(['timeout', 'network_error']), { restored: true, attempts: 3 });
  assert.deepEqual(await run(['invalid_data']), { restored: false, attempts: 1 });
  assert.deepEqual(await run(['network_error', 'network_error', 'network_error']), { restored: false, attempts: 3 });
});

test('About identifies the Linux port and relabels the original Windows requirements', () => {
  const source = '<h2 data-v-1234abcd>系统配置</h2>t=f(()=>k("about_content")?String(v("about_content")).trim():"")';
  const patched = patchAbout(source);
  assert.ok(patched.includes('Linux 社区移植版'));
  assert.ok(patched.includes('Windows 原版系统配置'));
  assert.ok(patched.includes('html.replace("系统配置","Windows 原版系统配置")'));
  assert.throws(() => patchAbout('<h2>系统配置</h2>'), /About system section changed/);
});

test('the document language is set to Simplified Chinese once', () => {
  assert.equal(patchIndexLanguage('<html lang="en">'), '<html lang="zh-CN">');
  assert.equal(patchIndexLanguage('<html lang="zh-CN">'), '<html lang="zh-CN">');
  assert.throws(() => patchIndexLanguage('<html>'), /index language/);
});

test('dock pet cursor follows the frontend-commanded kind', () => {
  const source = `.pet-dock{position:absolute;inset:0 auto auto 0}${PET_CURSOR_ANCHORS[0]}${PET_CURSOR_ANCHORS[1]}.reward-btn{cursor:pointer}`;
  const result = patchPetCursor(source);
  assert.ok(result.startsWith(source));
  assert.ok(result.includes(PET_CURSOR_MARKER));
  // The whole dock subtree resolves to the kind the frontend commands through
  // `set_native_input_cursor`; WebKitGTK then recomputes the same kind instead
  // of an arrow, so the two cursor writers no longer alternate.
  assert.ok(result.includes('html{--wuwa-pet-cursor:auto}'));
  assert.ok(result.includes('.pet-dock,.pet-dock *{cursor:var(--wuwa-pet-cursor,auto)}'));
  // The command hook writes into that property, so the stylesheet must not
  // hard-code a kind of its own.
  assert.ok(!result.slice(source.length).includes('cursor:grab}'));
  assert.throws(() => patchPetCursor(source.replace(PET_CURSOR_ANCHORS[0], '.cat-wrap{width:1px}')), /dock pet stylesheet rule changed/);
  assert.throws(() => patchPetCursor(`${source}${PET_CURSOR_MARKER}`), /already patched/);
  assert.throws(() => patchPetCursor(`${PET_CURSOR_ANCHORS[0]}${PET_CURSOR_ANCHORS[1]}/* trailing */`), /does not end with a rule/);
});
