// Patches applied to the official static frontend before it is embedded into
// the Tauri binary. Every patch fails loudly when its anchor disappears, so a
// new official release cannot silently produce a half-patched build.

export function patchDomesticSdkUrls(source) {
  if (!source.includes('const APP_REGION="CN"')) {
    throw new Error('Expected the official CN frontend. Extract the CN installer before building.');
  }
  let changes = 0;
  const patched = source.replace(
    /(isdk(?:Src|Css)=")(\/\/pro-cdn-sdk\.kurogame\.com\/pro\/web-sdk\/1\.x\/(?:kr-sdk\.js|style\.css)")/g,
    (_, prefix, url) => {
      changes += 1;
      return `${prefix}https:${url}`;
    },
  );
  if (changes !== 2) {
    throw new Error(`Expected 2 CN SDK URLs, found ${changes}. Reconcile the app version before building.`);
  }
  return patched;
}

const COOKIE_HELPERS = 'getCookie=e=>{const t=`kuro_${e}=`,n=document.cookie.split(";");for(let s=0;s<n.length;s+=1){let o=n[s];for(;o.charAt(0)===" ";)o=o.substring(1,o.length);if(o.indexOf(t)===0)return o.substring(t.length,o.length)}return null},setCookie=(e,t,n)=>{const s=new Date(Date.now()+n*24*60*60*1e3).toUTCString();document.cookie=`kuro_${e}=${t}; expires=${s}; path=/; SameSite=None; Secure`}';

// WebKitGTK does not expose cookies written by JavaScript on the tauri://
// origin, so the CN login token would disappear immediately after login and
// after every restart. Mirror every cookie helper value into localStorage,
// which WebKitGTK does persist for this origin, and fall back to it on reads.
export function patchCookieStorage(source) {
  if (source.split(COOKIE_HELPERS).length !== 2) {
    throw new Error('Original cookie helpers changed; reconcile auth storage before building.');
  }
  const replacement = 'getCookie=e=>{try{const t=`kuro_${e}=`,n=document.cookie.split(";");for(let s=0;s<n.length;s+=1){let o=n[s];for(;o.charAt(0)===" ";)o=o.substring(1,o.length);if(o.indexOf(t)===0)return o.substring(t.length,o.length)}}catch{}try{const s=localStorage.getItem(`wuwatappo-cookie-${e}`);if(!s)return null;const o=JSON.parse(s);if(!o||typeof o.value!=="string")return null;if(!(Number(o.expires)>Date.now())){localStorage.removeItem(`wuwatappo-cookie-${e}`);return null}return o.value}catch{return null}},setCookie=(e,t,n)=>{const s=new Date(Date.now()+n*24*60*60*1e3).toUTCString();try{document.cookie=`kuro_${e}=${t}; expires=${s}; path=/; SameSite=None; Secure`}catch{}try{const o=`wuwatappo-cookie-${e}`;n<0?localStorage.removeItem(o):localStorage.setItem(o,JSON.stringify({value:String(t),expires:Date.now()+n*24*60*60*1e3}))}catch{}}';
  return source.replace(COOKIE_HELPERS, replacement);
}

// The official main view only computes chest timers after the activity
// config's server time has been applied. On Linux the auth watcher can observe
// an already-hydrated session, leaving that flag false and both chest buttons
// permanently disabled. The timers themselves already fall back to Date.now(),
// so mark readiness when the view first computes them.
export function patchChestTimers(source) {
  const timer = 'function Un(){if(!A.isLoggedIn){Bt.value=0,Nt.value=0,Do.value=0,Lo.value=0;return}if(!eo.value)return;';
  const visibility = 'function Qr(e){return La.value?e||Cr.value?!0:P.value:!1}';
  const panes = 'const Zr=pe(()=>Jr.value==="right"?!1:Jr.value==="left"?Qr(!0):Qr(Vn.value)),ei=pe(()=>Jr.value==="left"?!1:(Jr.value==="right",Qr(Wn.value)));';
  if (source.split(timer).length !== 2 || source.split(visibility).length !== 2 || source.split(panes).length !== 2) {
    throw new Error('Chest timer or visibility code changed; reconcile the frontend before building.');
  }
  return source
    .replace(timer, 'function Un(){if(!A.isLoggedIn){Bt.value=0,Nt.value=0,Do.value=0,Lo.value=0;return}eo.value=!0;')
    .replace(visibility, 'function Qr(e){return !!(A.isLoggedIn&&Cr.value||La.value&&e)}')
    .replace(panes, 'const Zr=pe(()=>A.isLoggedIn&&Cr.value?!0:Jr.value==="right"?!1:Jr.value==="left"?Qr(!0):Qr(Vn.value)),ei=pe(()=>A.isLoggedIn&&Cr.value?!0:Jr.value==="left"?!1:(Jr.value==="right",Qr(Wn.value)));');
}

export function patchLoginReady(source) {
  const original = 'function N(n){return o(u,t=>{n(t.payload??{})})}';
  const replacement = 'async function N(n){const unlisten=await o(u,t=>{n(t.payload??{})});await window.__TAURI_INTERNALS__.invoke("login_window_ready");return unlisten}';
  if (source.split(original).length !== 2) {
    throw new Error('Original login listener bridge changed; reconcile login readiness before building.');
  }
  return source.replace(original, replacement);
}

export const RELAUNCH_SOURCE = 'async function relaunch(){await invoke("plugin:process|restart")}';

// The process plugin restarts from the Tauri main thread, which tears down
// WebKitGTK synchronously and freezes the app. Route the relaunch through a
// native command that spawns the next process before this one exits.
export function patchRelaunch(source) {
  if (source.split(RELAUNCH_SOURCE).length !== 2) {
    throw new Error('Original plugin process relaunch changed; reconcile the restart path before building.');
  }
  return source.replace(RELAUNCH_SOURCE, 'async function relaunch(){await invoke("restart_app")}');
}

export const SESSION_RESTORE_SOURCE = 'function an(e={}){return Z(),le("infoToken")?I||(I=(async()=>{try{return await pe(),e.emitLoginSuccess!==!1&&window.dispatchEvent(new CustomEvent("cat-auth-login-success")),!0}catch(n){return console.error("根据 infoToken 恢复登录态失败:",n),a("auth:session_restore_failed"),l("AUTH","operation_failed",{stage:"session_restore",reason_code:w(n)},"warn"),!1}finally{I=null}})(),I):Promise.resolve(!1)}';

// A cold start can race the network coming up. The official code treats every
// `user/me` failure as "no saved login", so a transient DNS/connect error hides
// a perfectly valid token and makes the app look logged out. Retry only the
// transport failures; auth rejections (10002/10003) must still log out at once.
export function patchSessionRestore(source) {
  if (source.split(SESSION_RESTORE_SOURCE).length !== 2) {
    throw new Error('Original session restore flow changed; reconcile auth retry before building.');
  }
  const replacement = 'function an(e={}){return Z(),le("infoToken")?I||(I=(async()=>{try{for(let t=0;t<3;t+=1){try{await pe();e.emitLoginSuccess!==!1&&window.dispatchEvent(new CustomEvent("cat-auth-login-success"));return!0}catch(n){const s=w(n);if(t<2&&(s==="network_error"||s==="timeout")){await new Promise(o=>setTimeout(o,1e3*(t+1)));continue}throw n}}return!1}catch(n){return console.error("根据 infoToken 恢复登录态失败:",n),a("auth:session_restore_failed"),l("AUTH","operation_failed",{stage:"session_restore",reason_code:w(n)},"warn"),!1}finally{I=null}})(),I):Promise.resolve(!1)}';
  return source.replace(SESSION_RESTORE_SOURCE, replacement);
}

export function patchAbout(source) {
  const headings = [...source.matchAll(/<h2 (data-v-[a-f0-9]+)>系统配置<\/h2>/g)];
  if (headings.length !== 1) {
    throw new Error('Original About system section changed; reconcile the Linux build information before building.');
  }
  const [heading, scope] = headings[0];
  const remotes = [...source.matchAll(/t=([$\w]+)\(\(\)=>k\("about_content"\)\?String\(v\("about_content"\)\)\.trim\(\):""\)/g)];
  if (remotes.length !== 1) {
    throw new Error('Original remote About content changed; reconcile the Linux build information before building.');
  }
  const [remote, computed] = remotes[0];
  const info = `<h2 ${scope}>Linux 社区移植版</h2><p ${scope}>Wuwa Tappo CN Linux · 非官方移植 ·</p>`;
  const banner = `<section class="about-section" ${scope}>${info}</section>`;
  return source
    .replace(heading, `${info}<h3 ${scope}>Windows 原版系统配置</h3>`)
    .replace(remote, `t=${computed}(()=>{const html=k("about_content")?String(v("about_content")).trim():"";return html?${JSON.stringify(banner)}+html.replace("系统配置","Windows 原版系统配置"):""})`);
}

export function patchIndexLanguage(index) {
  if (index.includes('<html lang="zh-CN">')) return index;
  if (!index.includes('<html lang="en">')) {
    throw new Error('Original index language attribute changed; reconcile the frontend before building.');
  }
  return index.replace('<html lang="en">', '<html lang="zh-CN">');
}
