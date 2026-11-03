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
  patchSessionRestore,
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

test('chest timers are not blocked by activity config readiness', () => {
  const source = 'function Vn(){if(!R.isLoggedIn){Nt.value=0,Ht.value=0,Po.value=0,To.value=0;return}if(!Qn.value)return;const e=is()}function Xr(e){return Ta.value?e||yr.value?!0:T.value:!1}const zr=pe(()=>Kr.value==="right"?!1:Kr.value==="left"?Xr(!0):Xr(Wn.value)),jr=pe(()=>Kr.value==="left"?!1:(Kr.value==="right",Xr(Gn.value)));';
  const patched = patchChestTimers(source);
  assert.ok(patched.includes('Qn.value=!0;'));
  assert.ok(!patched.includes('if(!Qn.value)return;'));
  assert.ok(patched.includes('!!(R.isLoggedIn&&yr.value||Ta.value&&e)'));
  assert.ok(patched.includes('const zr=pe(()=>R.isLoggedIn&&yr.value?!0:'));
  assert.throws(() => patchChestTimers('function Vn(){if(!Qn.value)return;}'), /Chest timer or visibility/);
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
  assert.throws(() => patchSessionRestore('function gn(e={}){}'), /session restore flow changed/);

  async function run(failures) {
    const attempts = [];
    const context = {
      D: null,
      ie: () => {},
      ge: () => 'token',
      Ke: async () => {
        attempts.push(1);
        const failure = failures[attempts.length - 1];
        if (failure) throw { reason: failure };
      },
      window: { dispatchEvent: () => {} },
      CustomEvent: class {},
      console: { error: () => {} },
      c: () => {},
      l: () => {},
      d: error => error.reason,
      setTimeout: callback => callback(),
      Promise,
      Error,
    };
    vm.runInNewContext(`${patched};globalThis.bootstrap=gn;`, context);
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
