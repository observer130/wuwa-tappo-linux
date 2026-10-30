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

// The 1.0.2 main view only computes chest timers after the activity config's
// server time has been applied. On Linux the auth watcher can observe an
// already-hydrated session, leaving that flag false and both chest buttons
// permanently disabled. The timers themselves already fall back to Date.now(),
// so mark readiness when the view first computes them.
export function patchChestTimers(source) {
  const timer = 'function Vn(){if(!A.isLoggedIn){Bt.value=0,Nt.value=0,Po.value=0,To.value=0;return}if(!Qn.value)return;';
  const visibility = 'function Xr(e){return Pa.value?e||yr.value?!0:T.value:!1}';
  const panes = 'const zr=pe(()=>Kr.value==="right"?!1:Kr.value==="left"?Xr(!0):Xr(Wn.value)),jr=pe(()=>Kr.value==="left"?!1:(Kr.value==="right",Xr(Gn.value)));';
  if (source.split(timer).length !== 2 || source.split(visibility).length !== 2 || source.split(panes).length !== 2) {
    throw new Error('Chest timer or visibility code changed; reconcile the frontend before building.');
  }
  return source
    .replace(timer, 'function Vn(){if(!A.isLoggedIn){Bt.value=0,Nt.value=0,Po.value=0,To.value=0;return}Qn.value=!0;')
    .replace(visibility, 'function Xr(e){return !!(A.isLoggedIn&&yr.value||Pa.value&&e)}')
    .replace(panes, 'const zr=pe(()=>A.isLoggedIn&&yr.value?!0:Kr.value==="right"?!1:Kr.value==="left"?Xr(!0):Xr(Wn.value)),jr=pe(()=>A.isLoggedIn&&yr.value?!0:Kr.value==="left"?!1:(Kr.value==="right",Xr(Gn.value)));');
}

export function patchLoginReady(source) {
  const original = 'function N(n){return o(u,t=>{n(t.payload??{})})}';
  const replacement = 'async function N(n){const unlisten=await o(u,t=>{n(t.payload??{})});await window.__TAURI_INTERNALS__.invoke("login_window_ready");return unlisten}';
  if (source.split(original).length !== 2) {
    throw new Error('Original login listener bridge changed; reconcile login readiness before building.');
  }
  return source.replace(original, replacement);
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
