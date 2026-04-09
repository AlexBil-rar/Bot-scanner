'use strict';

function fu(n) {
  return (n >= 0 ? '+' : '-') + '$' + Math.abs(n).toFixed(4);
}

function fuShort(n) {
  return (n >= 0 ? '+' : '-') + '$' + Math.abs(n).toFixed(2);
}

function typeLabel(t) {
  if (t === 'LIQUIDITY_RECOVERY') return '<span class="b bliq">LIQ</span>';
  if (t === 'STUCK_EXIT')         return '<span class="b bstuck">STUCK</span>';
  return t;
}

function load() {
  fetch('/trades_data')
    .then(r => r.json())
    .then(d => {
      if (!d) return;

      // ── Арбитражные метрики ──────────────────────────────────────
      const s = d.summary || {};

      document.getElementById('mt').textContent  = s.success || 0;

      const p  = s.total_profit || 0;
      const pe = document.getElementById('mp');
      pe.textContent = fu(p);
      pe.className   = 'metric-value ' + (p >= 0 ? 'green' : 'red');

      const avgEl = document.getElementById('ma');
      avgEl.textContent = fu(s.avg_profit || 0);
      avgEl.className   = 'metric-value ' + ((s.avg_profit || 0) >= 0 ? 'green' : 'red');

      document.getElementById('ml2').textContent = (s.avg_latency || 0).toFixed(0) + 'ms';
      document.getElementById('mv2').textContent = '$' + (s.total_volume || 0).toFixed(2);
      document.getElementById('mf').textContent  = s.buy_failed || 0;

      const denom = (s.total || 0) - (s.buy_failed || 0);
      document.getElementById('mw').textContent =
        denom > 0 ? (((s.success || 0) / denom) * 100).toFixed(0) + '%' : '—';

      // ── Ликвидность: счётчики ────────────────────────────────────
      const ls = d.liq_summary || {};
      const noLiq = !ls || ls.total_sales == null;

      const lsTotalEl = document.getElementById('ls-total');
      const lsPnlEl   = document.getElementById('ls-pnl');
      const lsAvgEl   = document.getElementById('ls-avg-pnl');
      const lsBrkEl   = document.getElementById('ls-breakdown');
      const lsTypEl   = document.getElementById('ls-types');
      const lsFailEl  = document.getElementById('ls-failed');

      if (noLiq) {
        lsTotalEl.textContent = '0';
        lsTypEl.textContent   = 'No data yet';
        lsPnlEl.textContent   = '+$0.00';
        lsAvgEl.textContent   = 'avg — ';
        lsBrkEl.innerHTML     = '<span style="color:var(--muted)">—</span>';
        lsFailEl.textContent  = '0';
      } else {
        lsTotalEl.textContent = ls.success_sales || 0;
        lsTypEl.textContent   =
          (ls.liq_recovery_cnt || 0) + ' LIQ · ' + (ls.stuck_exit_cnt || 0) + ' STUCK';

        const pnl = ls.total_pnl || 0;
        lsPnlEl.textContent = fuShort(pnl);
        lsPnlEl.className   = 'metric-value ' + (pnl >= 0 ? 'green' : 'red');

        const avgPnl = ls.avg_pnl || 0;
        lsAvgEl.innerHTML = 'avg <span class="' + (avgPnl >= 0 ? 'green' : 'red') + '">' + fu(avgPnl) + '</span>';

        lsBrkEl.innerHTML =
          `<div class="liq-row"><span class="green">✅ ${ls.pnl_positive || 0} profit</span></div>` +
          `<div class="liq-row"><span style="color:var(--muted)">➖ ${ls.pnl_zero || 0} zero</span></div>` +
          `<div class="liq-row"><span class="red">🔴 ${ls.pnl_negative || 0} loss</span></div>`;

        lsFailEl.textContent = ls.failed_sales || 0;
      }

      // ── Ликвидность: таблица по биржам ──────────────────────────
      const liqByExch = d.liq_by_exchange || [];
      document.getElementById('liq-by-exch').innerHTML = liqByExch.length === 0
        ? '<tr><td colspan="8" style="color:var(--muted);text-align:center;padding:16px">No liquidity sales yet</td></tr>'
        : liqByExch.map(r => {
            const pnl    = r.total_pnl || 0;
            const avgPnl = r.avg_pnl   || 0;
            const typeT  = r.sale_type === 'LIQUIDITY_RECOVERY' ? 'LIQ-REC' : 'STUCK';
            return `<tr>
              <td style="font-weight:500">${r.exchange}</td>
              <td><b>${r.symbol}</b></td>
              <td><span class="b ${r.sale_type === 'LIQUIDITY_RECOVERY' ? 'bliq' : 'bstuck'}">${typeT}</span></td>
              <td>${r.cnt}</td>
              <td class="${pnl >= 0 ? 'green' : 'red'}" style="font-weight:500">${fu(pnl)}</td>
              <td class="${avgPnl >= 0 ? 'green' : 'red'}">${fu(avgPnl)}</td>
              <td class="green">${r.profit_cnt || 0}</td>
              <td class="${(r.zero_or_loss_cnt || 0) > 0 ? 'amber' : ''}">${r.zero_or_loss_cnt || 0}</td>
            </tr>`;
          }).join('');

      // ── Ликвидность: история продаж ──────────────────────────────
      const liqSales = d.liq_sales || [];
      if (liqSales.length === 0) {
        document.getElementById('liq-history').innerHTML =
          '<p style="color:var(--muted);text-align:center;padding:16px">Liquidity sales will appear here when the bot sells crypto to recover stablecoins.</p>';
      } else {
        let lh = `<table><thead><tr>
          <th>Time</th><th>Type</th><th>Exchange</th><th>Symbol</th>
          <th>Qty</th><th>Buy $</th><th>Sell $</th>
          <th>Cost</th><th>Recv</th><th>P&amp;L</th><th>P&amp;L %</th><th>Status</th>
        </tr></thead><tbody>`;

        liqSales.forEach(r => {
          const tm  = new Date(r.timestamp_ms).toLocaleTimeString('ru-RU', { timeZone: 'Asia/Jerusalem' });
          const day = new Date(r.timestamp_ms).toLocaleDateString('ru-RU', {
            timeZone: 'Asia/Jerusalem', day: '2-digit', month: 'short',
          });
          const pnl    = r.pnl_usd  || 0;
          const pnlPct = r.pnl_pct  || 0;
          const noBuyPrice = !r.buy_price || r.buy_price === 0;

          lh += `<tr>
            <td style="color:var(--muted);white-space:nowrap">${day} ${tm}</td>
            <td>${typeLabel(r.sale_type)}</td>
            <td>${r.exchange}</td>
            <td><b>${r.symbol}</b></td>
            <td>${(r.qty || 0).toFixed(4)}</td>
            <td>${noBuyPrice ? '<span style="color:var(--muted)">—</span>' : '$' + (r.buy_price).toFixed(4)}</td>
            <td>${r.sell_price ? '$' + (r.sell_price).toFixed(4) : '<span style="color:var(--muted)">—</span>'}</td>
            <td>${r.buy_cost_usd ? '$' + (r.buy_cost_usd).toFixed(4) : '—'}</td>
            <td>${r.sell_recv_usd ? '$' + (r.sell_recv_usd).toFixed(4) : '—'}</td>
            <td class="${pnl > 0 ? 'green' : pnl < 0 ? 'red' : ''}" style="font-weight:500">
              ${noBuyPrice ? '<span style="color:var(--muted)">unknown</span>' : fu(pnl)}
            </td>
            <td class="${pnlPct > 0 ? 'green' : pnlPct < 0 ? 'red' : ''}">
              ${noBuyPrice ? '—' : (pnlPct >= 0 ? '+' : '') + pnlPct.toFixed(3) + '%'}
            </td>
            <td>
              ${r.status === 'SUCCESS'
                ? '<span class="b bok">OK</span>'
                : '<span class="b bfl">FAIL</span>'}
            </td>
          </tr>`;
        });
        lh += '</tbody></table>';
        document.getElementById('liq-history').innerHTML = lh;
      }

      // ── Арбитраж: по парам ───────────────────────────────────────
      document.getElementById('pt').innerHTML = (d.by_pair || []).map(r =>
        `<tr>
          <td style="font-weight:500">${r.pair}</td>
          <td>${r.success}/${r.total}</td>
          <td>$${(r.volume || 0).toFixed(2)}</td>
          <td class="${(r.profit || 0) >= 0 ? 'green' : 'red'}" style="font-weight:500">
            ${fu(r.profit || 0)}
          </td>
        </tr>`
      ).join('');

      // ── Арбитраж: история по дням ────────────────────────────────
      const trades = d.trades || [];
      const byDay  = {};
      trades.forEach(t => {
        const day = new Date(t.timestamp_ms).toLocaleDateString('ru-RU', {
          timeZone: 'Asia/Jerusalem',
          day: '2-digit', month: 'short', year: 'numeric',
        });
        if (!byDay[day]) byDay[day] = { trades: [], profit: 0, vol: 0, ok: 0, fail: 0, skip: 0 };
        byDay[day].trades.push(t);
        if (t.status === 'SUCCESS') {
          byDay[day].profit += t.profit_usd;
          byDay[day].vol    += t.trade_size_usd;
          byDay[day].ok++;
        } else if (t.status === 'BUY_FAILED') {
          byDay[day].skip++;
        } else {
          byDay[day].fail++;
        }
      });

      let h = '';
      for (const [day, dd] of Object.entries(byDay)) {
        h += `<div class="day-header">
          <span class="day-title">${day}</span>
          <div class="day-stats">
            <span>${dd.ok}/${dd.ok + dd.fail} trades</span>
            ${dd.skip ? `<span style="color:var(--amber)">${dd.skip} skipped</span>` : ''}
            <span>$${dd.vol.toFixed(2)}</span>
            <span class="${dd.profit >= 0 ? 'green' : 'red'}" style="font-weight:600">
              ${fu(dd.profit)}
            </span>
          </div>
        </div>`;

        h += `<table><thead><tr>
          <th>Time</th><th>Pair</th><th>Symbol</th>
          <th>Qty</th><th>Buy $</th><th>Sell $</th>
          <th>Size</th><th>Profit</th><th>Latency</th><th>Status</th>
        </tr></thead><tbody>`;

        dd.trades.forEach(t => {
          if (t.status !== 'SUCCESS') return;
          const tm   = new Date(t.timestamp_ms).toLocaleTimeString('ru-RU', { timeZone: 'Asia/Jerusalem' });
          const pair = t.buy_exchange + ' → ' + t.sell_exchange;
          h += `<tr>
            <td style="color:var(--muted)">${tm}</td>
            <td>${pair}</td>
            <td><b>${t.symbol}</b></td>
            <td>${t.quantity.toFixed(4)}</td>
            <td>$${t.buy_price.toFixed(4)}</td>
            <td>$${t.sell_price.toFixed(4)}</td>
            <td>$${t.trade_size_usd.toFixed(2)}</td>
            <td class="${t.profit_usd >= 0 ? 'green' : 'red'}" style="font-weight:500">
              ${fu(t.profit_usd)}
            </td>
            <td>${t.latency_ms}ms</td>
            <td><span class="b bok">OK</span></td>
          </tr>`;
        });
        h += '</tbody></table>';
      }

      document.getElementById('tl').innerHTML =
        h || '<p style="color:var(--muted);text-align:center;padding:20px">Trades will appear here as the bot executes them.</p>';
    });
}

load();
setInterval(load, 30_000);
