'use strict';

const charts = {};

function fmt(n, dec = 4) { return (+n).toFixed(dec); }
function fmtMs(ms) { return ms < 1000 ? ms + 'ms' : (ms / 1000).toFixed(1) + 's'; }
function fmtTime(ts) {
  return new Date(ts).toLocaleString('ru-RU', {
    timeZone: 'Asia/Jerusalem',
    day: '2-digit', month: '2-digit',
    hour: '2-digit', minute: '2-digit', second: '2-digit',
    hour12: false,
  });
}
function destroyChart(id) {
  if (charts[id]) { charts[id].destroy(); delete charts[id]; }
}

// ── Balances ──────────────────────────────────────────────────────────────

function loadBalances() {
  fetch('/balances')
    .then(r => r.json())
    .then(renderBalances)
    .catch(() => {
      document.getElementById('balances-content').innerHTML =
        '<div class="error">Ошибка загрузки балансов</div>';
    });
}

function renderBalances(b) {
  const exchanges = [
    { key: 'coinbase', name: 'Coinbase', currency: 'USD/USDC' },
    { key: 'binance',  name: 'Binance',  currency: 'USDT' },
    { key: 'bitget',   name: 'Bitget',   currency: 'USDT' },
    { key: 'bybit',    name: 'Bybit',    currency: 'USDT' },
  ];

  let html = '';
  let total = 0;
  let allLoaded = true;

  exchanges.forEach(ex => {
    const val = b[ex.key];
    if (val === null || val === undefined) {
      allLoaded = false;
      html += `<div class="balance-row">
        <span class="exchange-name">${ex.name}</span>
        <span class="error">нет данных</span>
      </div>`;
    } else {
      total += val;
      html += `<div class="balance-row">
        <span class="exchange-name">${ex.name}</span>
        <span>
          <span class="balance-amount">$${val.toFixed(2)}</span>
          <span class="balance-currency">${ex.currency}</span>
        </span>
      </div>`;
    }
  });

  if (allLoaded) {
    html += `<div class="balance-row" style="border-top:1px solid var(--border);margin-top:4px;padding-top:10px">
      <span class="exchange-name" style="color:var(--muted)">ИТОГО</span>
      <span class="balance-amount">$${total.toFixed(2)}</span>
    </div>`;

    fetch('/start_balance')
      .then(r => r.json())
      .then(sb => {
        if (sb && sb.total) {
          const totalProfit = total - sb.total;

          fetch('/data').then(r => r.json()).then(d => {
            const tradingProfit = (d && d.actual_profit_usd) || 0;
            const holdProfit = totalProfit - tradingProfit;

            const fmtP = v => (v >= 0 ? '+' : '-') + '$' + Math.abs(v).toFixed(2);
            const cls  = v => 'profit-box-value ' + (v >= 0 ? 'green' : 'red');

            const holdEl = document.getElementById('profit-hold');
            holdEl.textContent = fmtP(holdProfit);
            holdEl.className = cls(holdProfit);

            const tradingEl = document.getElementById('profit-trading');
            tradingEl.textContent = fmtP(tradingProfit);
            tradingEl.className = cls(tradingProfit);

            const totalEl = document.getElementById('profit-real');
            totalEl.textContent = fmtP(totalProfit);
            totalEl.className = cls(totalProfit);
          });

          document.getElementById('profit-real-sub').textContent =
            `старт: $${sb.total.toFixed(2)} → сейчас: $${total.toFixed(2)}`;
        } else {
          document.getElementById('profit-real-sub').textContent = 'стартовый баланс не задан';
        }
      });
  }

  const upd = new Date(b.updated_at * 1000).toLocaleTimeString('ru-RU');
  document.getElementById('balances-content').innerHTML =
    html + `<div style="font-size:11px;color:var(--muted);margin-top:8px">обновлено: ${upd}</div>`;
}

// ── Main data ─────────────────────────────────────────────────────────────

function loadData() {
  fetch('/data')
    .then(r => r.json())
    .then(d => { if (d) renderData(d); })
    .catch(e => console.error(e));
}

function renderData(d) {
  const s = d.stats;
  document.getElementById('m-total').textContent      = s.total || 0;
  document.getElementById('m-avg-profit').textContent = fmt(s.avg_profit || 0) + '%';
  document.getElementById('m-raw-spread').textContent = 'raw ' + fmt(s.avg_raw || 0) + '%';
  document.getElementById('m-max-profit').textContent = fmt(s.max_profit || 0) + '%';
  document.getElementById('profit-est').textContent   = '$' + fmt(s.real_profit_usd || 0, 2);
  document.getElementById('last-update').textContent  = new Date().toLocaleTimeString('ru-RU');

  if (d.opps.length) {
    const rate = d.opps[0].usdt_rate;
    const dev  = ((rate - 1) * 100).toFixed(4);
    document.getElementById('usdt-badge').textContent =
      `USDT ${rate} (${dev > 0 ? '+' : ''}${dev}%)`;
  }

  // Symbols chart
  destroyChart('sym');
  charts['sym'] = new Chart(document.getElementById('symChart'), {
    type: 'bar',
    data: {
      labels: d.by_sym.map(r => r.symbol),
      datasets: [{
        data: d.by_sym.map(r => r.cnt),
        backgroundColor: ['#22c55e','#3b82f6','#f59e0b','#a855f7','#ec4899'],
        borderRadius: 4, borderSkipped: false,
      }],
    },
    options: {
      responsive: true, maintainAspectRatio: false,
      plugins: { legend: { display: false } },
      scales: {
        x: { grid: { display: false }, ticks: { color: '#7b8099' } },
        y: { grid: { color: 'rgba(255,255,255,0.05)' }, ticks: { color: '#7b8099', stepSize: 1 } },
      },
    },
  });

  // Profit distribution
  const pb = { '0.01-0.02': 0, '0.02-0.04': 0, '0.04-0.07': 0, '>0.07': 0 };
  d.opps.forEach(o => {
    const p = o.profit_pct;
    if (p < 0.02)      pb['0.01-0.02']++;
    else if (p < 0.04) pb['0.02-0.04']++;
    else if (p < 0.07) pb['0.04-0.07']++;
    else               pb['>0.07']++;
  });
  destroyChart('profit');
  charts['profit'] = new Chart(document.getElementById('profitChart'), {
    type: 'bar',
    data: {
      labels: Object.keys(pb),
      datasets: [{ data: Object.values(pb), backgroundColor: '#3b82f6', borderRadius: 4, borderSkipped: false }],
    },
    options: {
      responsive: true, maintainAspectRatio: false,
      plugins: { legend: { display: false } },
      scales: {
        x: { grid: { display: false }, ticks: { color: '#7b8099' } },
        y: { grid: { color: 'rgba(255,255,255,0.05)' }, ticks: { color: '#7b8099', stepSize: 1 } },
      },
    },
  });

  // Duration doughnut
  destroyChart('dur');
  charts['dur'] = new Chart(document.getElementById('durChart'), {
    type: 'doughnut',
    data: {
      labels: Object.keys(d.dur_buckets),
      datasets: [{
        data: Object.values(d.dur_buckets),
        backgroundColor: ['#ef4444','#f59e0b','#22c55e','#3b82f6'],
        borderWidth: 0,
      }],
    },
    options: {
      responsive: true, maintainAspectRatio: false,
      plugins: { legend: { position: 'bottom', labels: { color: '#7b8099', font: { size: 11 }, boxWidth: 10, padding: 8 } } },
    },
  });

  // Spread chart
  const sp = d.spreads.length > 0 ? d.spreads.slice(0, 30).reverse() : d.opps.slice(0, 20).reverse();
  const THRESHOLD = 0.15;
  const rawVals  = sp.map(r => +fmt(r.raw_spread_pct));
  const normVals = sp.map(r => +fmt(r.normalized_spread_pct || r.raw_spread_pct * 0.78));
  const pointColors = rawVals.map(v => v >= THRESHOLD ? '#ffffff' : 'rgba(255,255,255,0.2)');
  const pointSizes  = rawVals.map(v => v >= THRESHOLD ? 6 : 2);

  destroyChart('spread');
  charts['spread'] = new Chart(document.getElementById('spreadChart'), {
    type: 'line',
    data: {
      labels: sp.map((_, i) => i + 1),
      datasets: [
        {
          label: 'Raw %', data: rawVals,
          borderColor: '#ef4444', backgroundColor: 'rgba(239,68,68,0.08)',
          tension: 0.3, fill: true,
          pointRadius: pointSizes, pointBackgroundColor: pointColors, pointBorderColor: pointColors,
        },
        {
          label: 'Norm %', data: normVals,
          borderColor: '#22c55e', backgroundColor: 'rgba(34,197,94,0.08)',
          tension: 0.3, fill: true, pointRadius: 2, pointBackgroundColor: '#22c55e',
        },
        {
          label: 'Порог 0.15%', data: sp.map(() => THRESHOLD),
          borderColor: 'rgba(255,255,255,0.35)', borderDash: [4, 4], borderWidth: 1,
          pointRadius: 0, fill: false, tension: 0,
        },
      ],
    },
    plugins: [{
      id: 'spreadLabels',
      afterDatasetsDraw(chart) {
        const ctx2 = chart.ctx;
        const meta = chart.getDatasetMeta(0);
        ctx2.save();
        meta.data.forEach((pt, i) => {
          const v = rawVals[i];
          const isPeak =
            (i === 0 || v >= rawVals[i - 1]) &&
            (i === rawVals.length - 1 || v >= rawVals[i + 1]) &&
            v > 0.015;
          const isThreshold = v >= THRESHOLD;
          if (!isPeak && !isThreshold) return;
          ctx2.font        = isThreshold ? 'bold 10px sans-serif' : '9px sans-serif';
          ctx2.fillStyle   = isThreshold ? '#ffffff' : 'rgba(255,255,255,0.4)';
          ctx2.textAlign   = 'center';
          ctx2.fillText(v.toFixed(3) + '%', pt.x, pt.y - 8);
        });
        ctx2.restore();
      },
    }],
    options: {
      responsive: true, maintainAspectRatio: false,
      plugins: {
        legend: { labels: { color: '#7b8099', font: { size: 11 }, boxWidth: 10 } },
        tooltip: {
          callbacks: {
            label: ctx => {
              const v = ctx.parsed.y;
              const status = v >= THRESHOLD ? ' ✅ ТОРГУЕМ' : ' ⬜ ниже порога';
              return ` ${ctx.dataset.label}: ${v.toFixed(4)}%${ctx.datasetIndex === 0 ? status : ''}`;
            },
          },
        },
      },
      scales: {
        x: { display: false },
        y: {
          grid: { color: 'rgba(255,255,255,0.05)' },
          ticks: { color: '#7b8099', callback: v => v.toFixed(3) + '%' },
        },
      },
    },
  });

  // Spread stats below chart
  if (sp.length > 0) {
    const lastRaw  = rawVals[rawVals.length - 1];
    const normLast = normVals[normVals.length - 1];
    const maxRaw   = Math.max(...rawVals);
    const premium  = lastRaw - normLast;
    const gap      = THRESHOLD - lastRaw;

    const curEl = document.getElementById('ss-current');
    curEl.textContent  = lastRaw.toFixed(4) + '%';
    curEl.style.color  = lastRaw >= THRESHOLD ? 'var(--green)' : 'var(--text)';
    document.getElementById('ss-max').textContent     = maxRaw.toFixed(4) + '%';
    document.getElementById('ss-premium').textContent = premium.toFixed(4) + '%';

    const gapEl = document.getElementById('ss-gap');
    if (gap <= 0) {
      gapEl.textContent  = '✅ выше порога';
      gapEl.style.color  = 'var(--green)';
    } else {
      gapEl.textContent  = '+' + gap.toFixed(4) + '%';
      gapEl.style.color  = gap < 0.05 ? 'var(--amber)' : 'var(--muted)';
    }
  }

  // Top pairs bars
  const pairEl = document.getElementById('pairs-bars');
  pairEl.innerHTML = '';
  const maxCnt = Math.max(...d.by_pair.map(r => r.cnt), 1);
  d.by_pair.slice(0, 8).forEach(r => {
    pairEl.innerHTML += `<div class="bar-row">
      <span class="bar-label">${r.pair} ${r.symbol}</span>
      <div class="bar-track"><div class="bar-fill" style="width:${r.cnt / maxCnt * 100}%"></div></div>
      <span class="bar-val">+${fmt(r.max_profit)}%</span>
    </div>`;
  });

  // Signals table
  const tbody = document.getElementById('signals-table');
  tbody.innerHTML = '';
  d.opps.slice(0, 20).forEach(o => {
    const sc = o.signal_status === 'strong' ? 'strong' : o.signal_status === 'medium' ? 'medium' : 'weak';
    tbody.innerHTML += `<tr>
      <td style="color:#7b8099">${fmtTime(o.timestamp_ms)}</td>
      <td>${o.buy_exchange} → ${o.sell_exchange}</td>
      <td>${o.symbol}</td>
      <td style="color:#22c55e;font-weight:500">+${fmt(o.profit_pct)}%</td>
      <td>${fmt(o.normalized_spread_pct)}%</td>
      <td>$${fmt(o.max_size_usd, 0)}</td>
      <td>${fmtMs(o.signal_duration_ms)}</td>
      <td>${o.signal_score}/10</td>
      <td><span class="badge badge-${sc}">${o.signal_status}</span></td>
    </tr>`;
  });
}

// ── Orders ────────────────────────────────────────────────────────────────

function loadOrders() {
  fetch('/orders')
    .then(r => r.json())
    .then(orders => {
      const tbody   = document.getElementById('orders-table');
      const countEl = document.getElementById('orders-count');
      if (!orders || orders.length === 0) {
        tbody.innerHTML = '<tr><td colspan="7" style="color:var(--muted);text-align:center">Нет исполненных ордеров</td></tr>';
        countEl.textContent = '0 ордеров';
        return;
      }
      countEl.textContent = orders.length + ' ордеров';
      tbody.innerHTML = orders.slice(0, 30).map(o => {
        const t         = new Date(o.time).toLocaleString('ru-RU', { timeZone: 'Asia/Jerusalem' });
        const sideColor = (o.side === 'BUY') ? 'var(--green)' : 'var(--red)';
        const sideText  = (o.side === 'BUY') ? '🟢 BUY' : '🔴 SELL';
        return `<tr>
          <td style="color:#7b8099;font-size:12px">${t}</td>
          <td>${o.exchange}</td>
          <td><b>${o.symbol}</b></td>
          <td style="color:${sideColor};font-weight:500">${sideText}</td>
          <td>${o.qty.toFixed(4)}</td>
          <td>$${o.price.toFixed(4)}</td>
          <td style="font-weight:500">$${o.total.toFixed(2)}</td>
        </tr>`;
      }).join('');
    })
    .catch(() => {
      document.getElementById('orders-table').innerHTML =
        '<tr><td colspan="7" style="color:var(--red)">Ошибка загрузки</td></tr>';
    });
}

// ── Boot ──────────────────────────────────────────────────────────────────

loadData();
setInterval(loadData, 10_000);

loadOrders();
setInterval(loadOrders, 120_000);

loadBalances();
setInterval(loadBalances, 60_000);
