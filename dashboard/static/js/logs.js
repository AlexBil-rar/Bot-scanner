'use strict';

let curFilter  = '';
let curPage    = 1;
let totalPages = 1;
let userScrolled = false;
let searchTimer  = null;

function esc(s) {
  return String(s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

function utcToIsrael(utcStr) {
  if (!utcStr) return '';
  const d = new Date(utcStr + 'Z');
  return d.toLocaleString('he-IL', {
    timeZone: 'Asia/Jerusalem',
    hour12: false,
    day: '2-digit', month: '2-digit',
    hour: '2-digit', minute: '2-digit', second: '2-digit',
  });
}

function setFilter(f, btn) {
  curFilter = f;
  curPage   = 1;
  document.querySelectorAll('.toolbar .fbtn').forEach(b => b.classList.remove('on'));
  btn.classList.add('on');
  reload();
}

function debounceSearch() {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(() => { curPage = 1; reload(); }, 300);
}

function goPage(p) {
  if (p < 1 || p > totalPages) return;
  curPage = p;
  reload();
}

function buildUrl() {
  const lines = document.getElementById('tail-lines').value;
  const q     = encodeURIComponent(document.getElementById('search').value);
  return `/logs_data?lines=${lines}&page=${curPage}&size=200&level=${curFilter}&q=${q}`;
}

function reload() {
  document.getElementById('status').textContent = 'загрузка...';
  fetch(buildUrl())
    .then(r => r.json())
    .then(render)
    .catch(err => {
      document.getElementById('lw').innerHTML =
        `<div style="color:var(--red);padding:20px">${esc(String(err))}</div>`;
      document.getElementById('status').textContent = 'ошибка';
    });
}

function render(d) {
  if (d.error) {
    document.getElementById('lw').innerHTML =
      `<div style="color:var(--red);padding:20px">${esc(d.error)}</div>`;
    document.getElementById('status').textContent = 'ошибка';
    return;
  }

  totalPages = d.pages || 1;
  curPage    = d.page  || 1;

  // Update pagination controls
  document.getElementById('pg-info').textContent =
    `стр. ${curPage} / ${totalPages}  (${d.total} строк)`;
  document.getElementById('btn-first').disabled = curPage <= 1;
  document.getElementById('btn-prev').disabled  = curPage <= 1;
  document.getElementById('btn-next').disabled  = curPage >= totalPages;
  document.getElementById('btn-last').disabled  = curPage >= totalPages;

  // Render log lines
  const lines = d.lines || [];
  if (lines.length === 0) {
    document.getElementById('lw').innerHTML =
      '<div style="color:var(--muted);text-align:center;padding:40px">Нет строк по фильтру</div>';
    document.getElementById('status').textContent = '0 строк';
    return;
  }

  let html = '';
  for (const l of lines) {
    const cls = l.level === 'ERROR' ? 'err-line' : l.level === 'WARN' ? 'warn-line' : '';
    html += `<div class="log-line ${cls}">
      <span class="log-ts">${esc(utcToIsrael(l.ts))}</span>
      <span class="log-lvl lvl-${l.level}">${l.level}</span>
      <span class="log-msg">${esc(l.msg)}</span>
    </div>`;
  }

  const wrap = document.getElementById('lw');
  wrap.innerHTML = html;

  document.getElementById('status').textContent =
    `${lines.length} строк на странице`;

  // Auto-scroll to bottom on last page if user hasn't scrolled up
  if (curPage === totalPages && !userScrolled) {
    wrap.scrollTop = wrap.scrollHeight;
  }
}

// Track scroll position
document.addEventListener('DOMContentLoaded', () => {
  const wrap = document.getElementById('lw');
  wrap.addEventListener('scroll', () => {
    const atBottom = wrap.scrollHeight - wrap.scrollTop - wrap.clientHeight < 50;
    userScrolled = !atBottom;
    // When user scrolls to bottom again on last page — re-enable auto-scroll
    if (atBottom) userScrolled = false;
  });
});

// Initial load + auto-refresh every 15s (only if on last page)
reload();
setInterval(() => {
  // Only auto-refresh if viewing the last page (newest logs)
  if (curPage === totalPages) reload();
}, 15_000);
