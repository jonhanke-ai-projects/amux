(function () {
  // Theme: the visitor picks system, light or dark. "system" follows the OS
  // and keeps following it. Runs synchronously in <head>, before first paint.
  var KEY = 'amux-theme';
  var root = document.documentElement;
  var mq = window.matchMedia ? window.matchMedia('(prefers-color-scheme: light)') : null;
  function getPref() {
    var v = null;
    try { v = localStorage.getItem(KEY); } catch (e) {}
    return v === 'light' || v === 'dark' ? v : 'system';
  }
  function resolve(pref) { return pref === 'system' ? (mq && mq.matches ? 'light' : 'dark') : pref; }
  function apply(pref) {
    var t = resolve(pref);
    root.setAttribute('data-theme', t);
    root.setAttribute('data-theme-pref', pref);
    root.style.colorScheme = t;
    var meta = document.querySelector('meta[name="theme-color"]');
    if (meta) meta.setAttribute('content', t === 'light' ? '#fbfaf7' : '#14171d');
    var label = 'Theme: ' + pref + (pref === 'system' ? ' (' + t + ')' : '');
    document.querySelectorAll('.theme-btn').forEach(function (b) {
      b.setAttribute('aria-label', label); b.setAttribute('title', label);
    });
    document.querySelectorAll('[data-theme-set]').forEach(function (b) {
      b.setAttribute('aria-checked', b.getAttribute('data-theme-set') === pref ? 'true' : 'false');
    });
  }
  var pref = getPref();
  var theme = resolve(pref);
  apply(pref);
  if (mq) {
    var onOs = function () { if (getPref() === 'system') apply('system'); };
    if (mq.addEventListener) mq.addEventListener('change', onOs); else if (mq.addListener) mq.addListener(onOs);
  }
  window.addEventListener('storage', function (e) { if (e.key === KEY) apply(getPref()); });

  document.addEventListener('DOMContentLoaded', function () {
    apply(getPref());
    document.querySelectorAll('.theme-pick').forEach(function (pick) {
      var btn = pick.querySelector('.theme-btn');
      var menu = pick.querySelector('.theme-menu');
      if (!btn || !menu) return;
      var items = Array.prototype.slice.call(menu.querySelectorAll('[data-theme-set]'));
      function open(focusItem) {
        menu.hidden = false; btn.setAttribute('aria-expanded', 'true');
        if (focusItem) (menu.querySelector('[aria-checked="true"]') || items[0]).focus();
      }
      function close(refocus) {
        if (menu.hidden) return;
        menu.hidden = true; btn.setAttribute('aria-expanded', 'false');
        if (refocus) btn.focus();
      }
      btn.addEventListener('click', function (e) {
        e.stopPropagation();
        if (menu.hidden) open(e.detail === 0); else close(false);
      });
      btn.addEventListener('keydown', function (e) {
        if (e.key === 'ArrowDown') { e.preventDefault(); open(true); }
      });
      items.forEach(function (it, i) {
        it.addEventListener('click', function () {
          var p = it.getAttribute('data-theme-set');
          try { if (p === 'system') localStorage.removeItem(KEY); else localStorage.setItem(KEY, p); } catch (err) {}
          apply(p);
          if (window.posthog) posthog.capture('theme_set', { theme_pref: p, theme: resolve(p) });
          close(true);
        });
        it.addEventListener('keydown', function (e) {
          if (e.key === 'ArrowDown') { e.preventDefault(); items[(i + 1) % items.length].focus(); }
          else if (e.key === 'ArrowUp') { e.preventDefault(); items[(i - 1 + items.length) % items.length].focus(); }
          else if (e.key === 'Tab') { close(false); }
        });
      });
      document.addEventListener('click', function (e) { if (!pick.contains(e.target)) close(false); });
      document.addEventListener('keydown', function (e) { if (e.key === 'Escape') close(true); });
    });
  });


  // GitHub star count for the header, cached an hour (the unauthenticated
  // API allows 60 requests an hour per IP). Fills any count still empty.
  document.addEventListener('DOMContentLoaded', function () {
    var el = document.getElementById('gh-stars');
    if (!el) return;
    function show(n) { if (n && !/\d/.test(el.textContent)) el.textContent = '\u2605 ' + n; }
    var cached = null;
    try { cached = JSON.parse(localStorage.getItem('amux-gh-stars') || 'null'); } catch (e) {}
    if (cached && Date.now() - cached.t < 3600000) { show(cached.n); return; }
    fetch('https://api.github.com/repos/mixpeek/amux').then(function (r) { return r.json(); }).then(function (d) {
      if (!d.stargazers_count) return;
      try { localStorage.setItem('amux-gh-stars', JSON.stringify({ n: d.stargazers_count, t: Date.now() })); } catch (e) {}
      show(d.stargazers_count);
    }).catch(function () { if (cached) show(cached.n); });
  });

  // PostHog analytics — async, non-blocking
  !function(t,e){var o,n,p,r;e.__SV||(window.posthog=e,e._i=[],e.init=function(i,s,a){function g(t,e){var o=e.split(".");2==o.length&&(t=t[o[0]],e=o[1]),t[e]=function(){t.push([e].concat(Array.prototype.slice.call(arguments,0)))}}(p=t.createElement("script")).type="text/javascript",p.crossOrigin="anonymous",p.async=!0,p.src=s.api_host.replace(".i.posthog.com","-assets.i.posthog.com")+"/static/array.js",(r=t.getElementsByTagName("script")[0]).parentNode.insertBefore(p,r);var u=e;for(void 0!==a?u=e[a]=[]:a="posthog",u.people=u.people||[],u.toString=function(t){var e="posthog";return"posthog"!==a&&(e+="."+a),t||(e+=" (stub)"),e},u.people.toString=function(){return u.people.toString(1)+" (stub)"},o="init capture register register_once register_for_session unregister unregister_for_session getFeatureFlag getFeatureFlagPayload isFeatureEnabled reloadFeatureFlags updateEarlyAccessFeatureEnrollment getEarlyAccessFeatures on onFeatureFlags onSessionId getSurveys getActiveMatchingSurveys renderSurvey canRenderSurvey getNextSurveyStep identify setPersonProperties group resetGroups setPersonPropertiesForFlags resetPersonPropertiesForFlags setGroupPropertiesForFlags resetGroupPropertiesForFlags reset get_distinct_id getGroups get_session_id get_session_replay_url alias set_config startSessionRecording stopSessionRecording sessionRecordingStarted captureException loadToolbar get_property getSessionProperty createPersonProfile opt_in_capturing opt_out_capturing has_opted_in_capturing has_opted_out_capturing clear_opt_in_out_capturing debug getPageViewId captureTrackedMutations setTrackedMutations".split(" "),n=0;n<o.length;n++)g(u,o[n]);e._i.push([i,s,a])},e.__SV=1)}(document,window.posthog||[]);
  posthog.init('phc_Ckeacj8y8X8YiLkwHBcpBsJVSnsFKohch2vMv9sJYmE6', {
    api_host: 'https://us.i.posthog.com',
    person_profiles: 'identified_only',
    capture_pageview: true,
    capture_pageleave: true,
    enable_heatmaps: true,
  });
  posthog.register({ theme_preference: pref, theme: theme });

})();
