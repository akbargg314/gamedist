// Downloads page: lists the files of the latest GameDist release.
//
// Runs in the visitor's browser and asks the public GitHub API (no token)
// for the repository's releases. Nothing is hard-coded about versions or
// file names: the page shows whatever the latest release contains, grouped
// by program and platform. Without a release, or if the request fails, it
// says so and points at "Build from source". All text from the API is
// inserted with textContent, never as HTML.
(function () {
  'use strict';

  var REPO = 'akbargg314/gamedist';
  var API = 'https://api.github.com/repos/' + REPO + '/releases?per_page=20';
  var RELEASES_PAGE = 'https://github.com/' + REPO + '/releases';
  var TIMEOUT_MS = 10000;

  var box = document.getElementById('release');
  if (!box || !window.fetch) return; // keep the no-JavaScript text

  // ---------- small DOM helpers ----------

  function el(tag, attrs, children) {
    var node = document.createElement(tag);
    if (attrs) {
      for (var k in attrs) {
        if (!Object.prototype.hasOwnProperty.call(attrs, k)) continue;
        if (k === 'text') node.textContent = attrs[k];
        else if (k === 'class') node.className = attrs[k];
        else node.setAttribute(k, attrs[k]);
      }
    }
    (children || []).forEach(function (c) {
      if (c == null) return;
      node.appendChild(typeof c === 'string' ? document.createTextNode(c) : c);
    });
    return node;
  }

  function link(href, text, cls) {
    return el('a', { href: href, text: text, class: cls || '' });
  }

  function render(children) {
    box.textContent = '';
    box.appendChild(el('h2', { id: 'release-title', text: 'Latest release' }));
    children.forEach(function (c) { if (c) box.appendChild(c); });
  }

  // Only follow links that point at GitHub; the API is trusted for data, but
  // there is no reason to render any other kind of URL.
  function safeGithubUrl(u) {
    return typeof u === 'string' && /^https:\/\/github\.com\//.test(u) ? u : null;
  }

  function formatSize(bytes) {
    if (typeof bytes !== 'number' || !isFinite(bytes) || bytes < 0) return '';
    var units = ['bytes', 'KiB', 'MiB', 'GiB'];
    var i = 0;
    var v = bytes;
    while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
    return (i === 0 ? String(v) : v.toFixed(v >= 100 ? 0 : 1)) + ' ' + units[i];
  }

  function formatDate(iso) {
    var d = new Date(iso);
    return isNaN(d.getTime()) ? '' : d.toISOString().slice(0, 10);
  }

  // ---------- classifying assets ----------

  var OS_NAMES = { linux: 'Linux', windows: 'Windows', macos: 'macOS' };
  var ARCH_NAMES = { x86_64: 'x86_64', aarch64: 'ARM64 (aarch64)' };

  function osOf(name) {
    var n = name.toLowerCase();
    if (/windows|win64|\.msi$|\.exe$/.test(n)) return 'windows';
    if (/macos|darwin|apple|\.dmg$|\.app\.tar\.gz$/.test(n)) return 'macos';
    if (/linux|\.deb$|\.rpm$|\.appimage$/.test(n)) return 'linux';
    return null;
  }

  function archOf(name) {
    var n = name.toLowerCase();
    if (/aarch64|arm64/.test(n)) return 'aarch64';
    if (/x86_64|amd64|x64/.test(n)) return 'x86_64';
    return null;
  }

  var GROUPS = [
    {
      id: 'desktop',
      title: 'Desktop launcher',
      desc: 'Installable packages of the desktop app (gamedist-launcher).',
      test: function (n) { return /\.(deb|rpm|appimage|msi|dmg)$/i.test(n) || /-setup\.exe$/i.test(n) || /\.app\.tar\.gz$/i.test(n); }
    },
    {
      id: 'tools',
      title: 'Server, publisher CLI and headless launcher',
      desc: 'Each archive, as built by the current release workflow, contains gamedist-server, gamedist and gamedist-launcher (headless mode only), plus LICENSE and README. The Linux archive also has the deploy files.',
      test: function (n) { return /^gamedist-[a-z0-9]+-[a-z0-9_]+\.(tar\.gz|tgz|zip)$/i.test(n); }
    },
    {
      id: 'checksums',
      title: 'Checksums and signatures',
      desc: 'Check a download with: sha256sum -c SHA256SUMS --ignore-missing',
      test: function (n) { return /^sha256sums/i.test(n) || /\.(sha256|sig|asc|minisig)$/i.test(n); }
    },
    {
      id: 'other',
      title: 'Other files',
      desc: '',
      test: function () { return true; }
    }
  ];

  function groupOf(name) {
    for (var i = 0; i < GROUPS.length; i++) if (GROUPS[i].test(name)) return GROUPS[i].id;
    return 'other';
  }

  // ---------- the visitor's system ----------

  function detectSystem() {
    var nav = window.navigator;
    var uad = nav.userAgentData;
    var platform = ((uad && uad.platform) || nav.platform || '').toLowerCase();
    var ua = (nav.userAgent || '').toLowerCase();
    var sys = { os: null, arch: null, mobile: false };
    if (/android|iphone|ipad|ipod/.test(ua) || (uad && uad.mobile)) sys.mobile = true;
    if (/win/.test(platform) || /windows/.test(ua)) sys.os = 'windows';
    else if (/mac/.test(platform) || /mac os x|macintosh/.test(ua)) sys.os = 'macos';
    else if (/linux|x11|cros/.test(platform) || /linux|x11|cros/.test(ua)) sys.os = 'linux';
    // macOS browsers report "Intel" even on Apple silicon, so only trust the
    // user agent string for the architecture elsewhere.
    if (sys.os !== 'macos') {
      if (/aarch64|arm64|armv8/.test(ua)) sys.arch = 'aarch64';
      else if (/x86_64|x64|win64|wow64|amd64/.test(ua)) sys.arch = 'x86_64';
    }
    if (sys.mobile) sys.os = null;
    return sys;
  }

  function refineArch(sys) {
    var uad = window.navigator.userAgentData;
    if (!uad || typeof uad.getHighEntropyValues !== 'function') return Promise.resolve(sys);
    return uad.getHighEntropyValues(['architecture', 'bitness']).then(function (v) {
      if (v.architecture === 'arm' && v.bitness === '64') sys.arch = 'aarch64';
      else if (v.architecture === 'x86' && v.bitness === '64') sys.arch = 'x86_64';
      return sys;
    }, function () { return sys; });
  }

  function suggest(assets, sys) {
    if (!sys.os) return null;
    var candidates = assets.filter(function (a) {
      return a.os === sys.os && (a.group === 'desktop' || a.group === 'tools');
    });
    if (!candidates.length) return null;
    function score(a) {
      var s = 0;
      if (a.group === 'desktop') s += 10;
      if (sys.arch && a.arch === sys.arch) s += 5;
      else if (sys.arch && a.arch && a.arch !== sys.arch) s -= 20;
      else if (!sys.arch && a.arch === (sys.os === 'macos' ? 'aarch64' : 'x86_64')) s += 1;
      if (sys.os === 'linux' && /\.appimage$/i.test(a.name)) s += 2; // runs on any distribution
      if (sys.os === 'windows' && /-setup\.exe$/i.test(a.name)) s += 1;
      return s;
    }
    candidates.sort(function (x, y) { return score(y) - score(x); });
    return score(candidates[0]) > -10 ? candidates[0] : null;
  }

  function systemLabel(sys) {
    var label = OS_NAMES[sys.os] || 'your system';
    if (sys.arch) label += ' on ' + ARCH_NAMES[sys.arch];
    return label;
  }

  // ---------- rendering ----------

  function fallbackParagraphs(prefix) {
    return [
      prefix,
      el('p', null, [
        'Releases appear on the ',
        link(RELEASES_PAGE, 'releases page on GitHub'),
        '. Meanwhile you can ',
        link('#source', 'build from source'),
        '.'
      ])
    ];
  }

  function renderNoRelease() {
    render(fallbackParagraphs(el('p', null, [el('strong', { text: 'No release has been published yet.' })])));
  }

  function renderError(reason) {
    render(fallbackParagraphs(el('p', null, [
      'The release list could not be loaded from GitHub (' + reason + '). ',
      'When this page was written, no release had been published yet.'
    ])));
  }

  function assetRow(a, suggested) {
    var href = safeGithubUrl(a.url);
    var platform = a.os ? OS_NAMES[a.os] + (a.arch ? ', ' + ARCH_NAMES[a.arch] : '') : '—';
    var tr = el('tr', suggested ? { class: 'is-suggested' } : null, [
      el('td', null, [href ? link(href, a.name) : a.name, suggested ? el('span', { class: 'visually-hidden', text: ' (suggested)' }) : null]),
      el('td', { text: platform }),
      el('td', { class: 'num', text: formatSize(a.size) })
    ]);
    return tr;
  }

  function renderRelease(rel, sys, newerPrerelease) {
    var assets = (rel.assets || [])
      .filter(function (a) { return a && typeof a.name === 'string'; })
      .map(function (a) {
        return {
          name: a.name,
          size: a.size,
          url: a.browser_download_url,
          os: osOf(a.name),
          arch: archOf(a.name),
          group: groupOf(a.name)
        };
      });

    var nodes = [];
    var title = el('p', { class: 'release-title-line' }, [
      el('strong', { text: 'GameDist ' + (rel.name || rel.tag_name || '') }),
      rel.prerelease ? el('span', { class: 'badge', text: 'Pre-release' }) : null
    ]);
    nodes.push(title);

    var meta = [];
    var date = formatDate(rel.published_at);
    if (rel.tag_name) meta.push('Tag ' + rel.tag_name);
    if (date) meta.push('published ' + date);
    var notes = safeGithubUrl(rel.html_url);
    nodes.push(el('p', { class: 'release-meta' }, [
      meta.join(', ') + (meta.length ? '. ' : ''),
      notes ? link(notes, 'Release notes on GitHub') : null
    ]));

    if (newerPrerelease) {
      var preUrl = safeGithubUrl(newerPrerelease.html_url);
      nodes.push(el('p', { class: 'release-meta' }, [
        'A newer pre-release, ' + newerPrerelease.tag_name + ', is also available',
        preUrl ? ': ' : '.',
        preUrl ? link(preUrl, 'see it on GitHub') : null,
        preUrl ? '.' : null
      ]));
    }

    if (!assets.length) {
      nodes.push(el('p', null, [
        'This release has no files attached (yet). Check the ',
        link(RELEASES_PAGE, 'releases page'),
        ' or ',
        link('#source', 'build from source'),
        '.'
      ]));
      render(nodes);
      return;
    }

    var hasDesktop = assets.some(function (a) { return a.group === 'desktop'; });
    var pick = suggest(assets, sys);

    if (sys.mobile) {
      nodes.push(el('div', { class: 'suggest' }, [
        el('p', { class: 'suggest-title', text: 'GameDist runs on desktop systems.' }),
        el('p', { text: 'There are no builds for phones or tablets. The files below are for Linux, Windows and macOS.' })
      ]));
    } else if (pick) {
      var pickHref = safeGithubUrl(pick.url);
      var why = [];
      if (pick.group === 'tools') {
        why.push(hasDesktop
          ? 'There is no desktop app package for your system in this release, so this is the archive with the headless launcher, the server and the CLI.'
          : 'This release has no desktop app packages. This archive contains the headless launcher, the server and the CLI; for the desktop app, build from source.');
      }
      if (pick.os === 'macos' && pick.arch === 'aarch64') why.push('Built for Apple silicon; it does not run on Intel Macs.');
      else if (!sys.arch && pick.arch) why.push('Your browser does not report the processor type; this build is for ' + ARCH_NAMES[pick.arch] + '.');
      if (pick.os !== 'linux') why.push('Builds for ' + OS_NAMES[pick.os] + ' have not been tested.');
      nodes.push(el('div', { class: 'suggest' }, [
        el('p', { class: 'suggest-title', text: 'Suggested for ' + systemLabel(sys) }),
        el('p', null, [
          pickHref
            ? el('a', { class: 'button', href: pickHref }, [pick.name, el('span', { class: 'size', text: formatSize(pick.size) })])
            : pick.name
        ]),
        why.length ? el('p', { class: 'group-desc', text: why.join(' ') }) : null
      ]));
    } else if (sys.os) {
      nodes.push(el('div', { class: 'suggest' }, [
        el('p', { class: 'suggest-title', text: 'No file in this release matches ' + systemLabel(sys) + '.' }),
        el('p', null, ['You can ', link('#source', 'build from source'), '.'])
      ]));
    }

    GROUPS.forEach(function (g) {
      var items = assets.filter(function (a) { return a.group === g.id; });
      if (!items.length) return;
      var order = { linux: 0, windows: 1, macos: 2 };
      items.sort(function (x, y) {
        var ox = x.os in order ? order[x.os] : 9;
        var oy = y.os in order ? order[y.os] : 9;
        return ox - oy || (x.name < y.name ? -1 : x.name > y.name ? 1 : 0);
      });
      var table = el('table', null, [
        el('caption', { text: g.title }),
        el('thead', null, [el('tr', null, [
          el('th', { scope: 'col', text: 'File' }),
          el('th', { scope: 'col', text: 'Platform' }),
          el('th', { scope: 'col', class: 'num', text: 'Size' })
        ])]),
        el('tbody', null, items.map(function (a) { return assetRow(a, a === pick); }))
      ]);
      if (g.desc) nodes.push(el('p', { class: 'group-desc', text: g.desc }));
      nodes.push(el('div', { class: 'table-scroll' }, [table]));
    });

    if (!hasDesktop) {
      nodes.push(el('p', { class: 'group-desc' }, [
        'No desktop app packages are attached to this release. To use the desktop app, ',
        link('#source', 'build it from source'),
        '.'
      ]));
    }

    render(nodes);
  }

  // ---------- load ----------

  render([el('p', { text: 'Checking GitHub for the latest release…' })]);
  box.setAttribute('aria-busy', 'true');

  var controller = typeof AbortController === 'function' ? new AbortController() : null;
  var timer = controller ? setTimeout(function () { controller.abort(); }, TIMEOUT_MS) : null;

  var sysPromise = refineArch(detectSystem());

  fetch(API, {
    headers: { Accept: 'application/vnd.github+json' },
    referrerPolicy: 'no-referrer',
    credentials: 'omit',
    signal: controller ? controller.signal : undefined
  }).then(function (res) {
    if (res.status === 404) return [];
    if (!res.ok) {
      var limited = (res.status === 403 || res.status === 429) && res.headers.get('x-ratelimit-remaining') === '0';
      throw new Error(limited ? 'GitHub API rate limit reached; try again later' : 'HTTP ' + res.status);
    }
    return res.json();
  }).then(function (list) {
    if (!Array.isArray(list)) throw new Error('unexpected response');
    var published = list.filter(function (r) { return r && !r.draft; });
    published.sort(function (a, b) { return String(b.published_at).localeCompare(String(a.published_at)); });
    if (!published.length) return sysPromise.then(renderNoRelease);
    var stable = published.filter(function (r) { return !r.prerelease; })[0];
    var chosen = stable || published[0];
    var newerPre = stable && published[0] !== stable ? published[0] : null;
    return sysPromise.then(function (sys) { renderRelease(chosen, sys, newerPre); });
  }).catch(function (err) {
    var reason = err && err.name === 'AbortError' ? 'the request timed out' : (err && err.message) || 'network error';
    if (reason === 'Failed to fetch' || reason === 'NetworkError when attempting to fetch resource.' || reason === 'Load failed') reason = 'network error';
    renderError(reason);
  }).then(function () {
    if (timer) clearTimeout(timer);
    box.removeAttribute('aria-busy');
  });
})();
