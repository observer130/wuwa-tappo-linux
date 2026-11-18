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
  patchRelaunch,
  patchSessionRestore,
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

const CHEST_SOURCE = 'function Un(){if(!A.isLoggedIn){Bt.value=0,Nt.value=0,Do.value=0,Lo.value=0;return}if(!eo.value)return;const e=fs(),t=performance.now();Bt.value=Ir===null?ho(A.userInfo?.left_treasure_chest,e):Math.max(0,Ir-t),Nt.value=Mr===null?ho(A.userInfo?.right_treasure_chest,e):Math.max(0,Mr-t),$r("left",Bt.value),$r("right",Nt.value)}function Qr(e){return La.value?e||Cr.value?!0:P.value:!1}const Zr=pe(()=>Jr.value==="right"?!1:Jr.value==="left"?Qr(!0):Qr(Vn.value)),ei=pe(()=>Jr.value==="left"?!1:(Jr.value==="right",Qr(Wn.value)));';

test('chest timers are not blocked by activity config readiness', () => {
  const patched = patchChestTimers(CHEST_SOURCE);
  assert.ok(patched.includes('eo.value=!0;'));
  assert.ok(!patched.includes('if(!eo.value)return;'));
  assert.ok(patched.includes('!!((A.isLoggedIn&&Cr.value)||La.value&&(e||P.value))'));
  assert.ok(patched.includes('const Zr=pe(()=>A.isLoggedIn&&Cr.value?!0:'));
  assert.throws(() => patchChestTimers('function Un(){if(!eo.value)return;}'), /Chest timer or visibility/);

  const ref = value => ({ value });
  const timerState = {
    A: { isLoggedIn: true, userInfo: { left_treasure_chest: 111, right_treasure_chest: 222 } },
    eo: ref(false),
    Bt: ref(0),
    Nt: ref(0),
    Do: ref(0),
    Lo: ref(0),
    Ir: null,
    Mr: null,
    fs: () => 1000,
    performance: { now: () => 500 },
    ho: value => value,
    $r: () => {},
  };
  const timerSnippet = patched.slice(patched.indexOf('function Un('), patched.indexOf('function Qr('));
  vm.runInNewContext(`${timerSnippet};globalThis.runTimer=Un;`, timerState);
  timerState.runTimer();
  assert.equal(timerState.eo.value, true, 'readiness is marked on first computation');
  assert.equal(timerState.Bt.value, 111, 'left timer falls back to local computation');
  assert.equal(timerState.Nt.value, 222, 'right timer falls back to local computation');

  timerState.A.isLoggedIn = false;
  timerState.Bt.value = 9;
  timerState.Nt.value = 9;
  timerState.runTimer();
  assert.equal(timerState.Bt.value, 0);
  assert.equal(timerState.Nt.value, 0);
});

test('chest panes follow official visibility rules with port always-show', () => {
  const patched = patchChestTimers(CHEST_SOURCE);
  const snippet = patched.slice(patched.indexOf('function Qr(e)'));
  const ref = value => ({ value });
  const state = {
    A: { isLoggedIn: true },
    La: ref(true),
    Cr: ref(false),
    P: ref(false),
    Vn: ref(false),
    Wn: ref(false),
    Jr: ref(null),
    pe: compute => ({ get value() { return compute(); } }),
  };
  vm.runInNewContext(`${snippet};globalThis.panes=()=>[Zr.value,ei.value];`, state);
  const panesWith = overrides => {
    const values = {
      isLoggedIn: true, La: true, Cr: false, P: false, Vn: false, Wn: false, Jr: null,
      ...overrides,
    };
    state.A.isLoggedIn = values.isLoggedIn;
    state.La.value = values.La;
    state.Cr.value = values.Cr;
    state.P.value = values.P;
    state.Vn.value = values.Vn;
    state.Wn.value = values.Wn;
    state.Jr.value = values.Jr;
    return [...state.panes()];
  };

  // Official: hovering the pet reveals the countdown panes.
  assert.deepEqual(panesWith({ P: true }), [true, true]);
  assert.deepEqual(panesWith({}), [false, false]);
  // A ready chest stays visible without hover, on either side.
  assert.deepEqual(panesWith({ Vn: true }), [true, false]);
  assert.deepEqual(panesWith({ Wn: true }), [false, true]);
  assert.deepEqual(panesWith({ Vn: true, Wn: true }), [true, true]);
  // The balance gate hides panes even when hovering or a chest is ready.
  assert.deepEqual(panesWith({ La: false, P: true }), [false, false]);
  assert.deepEqual(panesWith({ La: false, Vn: true, Wn: true }), [false, false]);
  // "Always show chest" bypasses the balance gate when logged in.
  assert.deepEqual(panesWith({ Cr: true, La: false }), [true, true]);
  // Below 2000 clicks the official view shows only the affordable side.
  assert.deepEqual(panesWith({ Jr: 'right', Vn: false, Wn: true }), [false, true]);
  assert.deepEqual(panesWith({ Jr: 'left', Vn: true, Wn: false }), [true, false]);
  // Logged-out sessions keep the official two-button login prompt.
  assert.deepEqual(panesWith({ isLoggedIn: false, Vn: true, Wn: true }), [true, true]);
  assert.deepEqual(panesWith({ isLoggedIn: false, Cr: true, Vn: true, Wn: true }), [true, true]);
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
      Z: () => {},
      le: () => 'token',
      pe: async () => {
        attempts.push(1);
        const failure = failures[attempts.length - 1];
        if (failure) throw { reason: failure };
      },
      window: { dispatchEvent: () => {} },
      CustomEvent: class {},
      console: { error: () => {} },
      a: () => {},
      l: () => {},
      w: error => error.reason,
      setTimeout: callback => callback(),
      Promise,
      Error,
    };
    vm.runInNewContext(`${patched};globalThis.bootstrap=an;`, context);
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
