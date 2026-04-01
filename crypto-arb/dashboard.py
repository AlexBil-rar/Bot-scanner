#!/usr/bin/env python3
"""
Арбитраж дашборд — читает arb_data.db и открывает браузер
Запуск: python3 dashboard.py
"""
import sqlite3, json, sys, os, webbrowser, http.server, threading
from pathlib import Path

DB = "arb_data.db"
PORT = 8765

def get_data():
    if not Path(DB).exists():
        return None

    # Читаем капитал из .env
    capital = 300.0
    env_file = Path('.env')
    if not env_file.exists():
        env_file = Path('.env.example')
    if env_file.exists():
        for line in env_file.read_text().splitlines():
            if line.startswith('CAPITAL_PER_EXCHANGE='):
                try:
                    capital = float(line.split('=')[1].strip())
                except:
                    pass

    conn = sqlite3.connect(DB)
    conn.row_factory = sqlite3.Row

    opps = [dict(r) for r in conn.execute("""
        SELECT * FROM opportunities
        WHERE profit_pct >= 0.01
        ORDER BY timestamp_ms DESC
        LIMIT 1000
    """).fetchall()]

    spreads = [dict(r) for r in conn.execute("""
        SELECT * FROM spreads ORDER BY timestamp_ms DESC LIMIT 500
    """).fetchall()]

    stats = dict(conn.execute(f"""
        SELECT
            COUNT(*) as total,
            AVG(profit_pct) as avg_profit,
            MAX(profit_pct) as max_profit,
            AVG(signal_duration_ms) as avg_dur,
            AVG(normalized_spread_pct) as avg_norm,
            AVG(raw_spread_pct) as avg_raw,
            SUM(expected_profit_usd) as total_exp_usd,
            SUM(MIN({capital}, max_size_usd) * profit_pct / 100.0) as real_profit_usd
        FROM (
            SELECT buy_exchange, sell_exchange, symbol,
                MAX(profit_pct) as profit_pct,
                MAX(normalized_spread_pct) as normalized_spread_pct,
                MAX(raw_spread_pct) as raw_spread_pct,
                MAX(signal_duration_ms) as signal_duration_ms,
                MAX(expected_profit_usd) as expected_profit_usd,
                MAX(max_size_usd) as max_size_usd,
                signal_status
            FROM opportunities
            WHERE profit_pct >= 0.01 AND signal_status != 'weak'
            GROUP BY buy_exchange, sell_exchange, symbol,
                     CAST(timestamp_ms / 30000 AS INTEGER)
        )
    """).fetchone())

    by_sym = [dict(r) for r in conn.execute("""
        SELECT symbol,
            COUNT(*) as cnt,
            AVG(profit_pct) as avg_profit,
            MAX(profit_pct) as max_profit,
            AVG(signal_duration_ms)/1000.0 as avg_dur_s,
            SUM(expected_profit_usd) as total_exp
        FROM opportunities WHERE profit_pct >= 0.01 AND signal_status != 'weak'
        GROUP BY symbol ORDER BY cnt DESC
    """).fetchall()]

    by_pair = [dict(r) for r in conn.execute("""
        SELECT buy_exchange || '→' || sell_exchange as pair,
            symbol, COUNT(*) as cnt,
            AVG(profit_pct) as avg_profit,
            MAX(profit_pct) as max_profit,
            AVG(signal_duration_ms)/1000.0 as avg_dur_s
        FROM opportunities WHERE profit_pct >= 0.01 AND signal_status != 'weak'
        GROUP BY pair, symbol ORDER BY cnt DESC LIMIT 10
    """).fetchall()]

    # Duration buckets
    dur_data = conn.execute("""
        SELECT signal_duration_ms FROM opportunities
        WHERE profit_pct >= 0.01 AND signal_status != 'weak'
    """).fetchall()
    buckets = {'<1s':0,'1-5s':0,'5-15s':0,'>15s':0}
    for (d,) in dur_data:
        if d < 1000: buckets['<1s'] += 1
        elif d < 5000: buckets['1-5s'] += 1
        elif d < 15000: buckets['5-15s'] += 1
        else: buckets['>15s'] += 1

    conn.close()
    return {
        'opps': opps[:50],
        'spreads': spreads,
        'stats': stats,
        'capital': capital,
        'by_sym': by_sym,
        'by_pair': by_pair,
        'dur_buckets': buckets,
        'total_rows': len(opps),
    }

HTML = '''<!DOCTYPE html>
<html lang="ru">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Crypto Arb Dashboard</title>
<script src="https://cdn.jsdelivr.net/npm/chart.js@4.4.1/dist/chart.umd.min.js"></script>
<style>
  :root{--bg:#0f1117;--bg2:#1a1d27;--bg3:#242736;--border:#2e3347;--text:#e8eaf0;--muted:#7b8099;--green:#22c55e;--blue:#3b82f6;--amber:#f59e0b;--red:#ef4444}
  *{box-sizing:border-box;margin:0;padding:0}
  body{background:var(--bg);color:var(--text);font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;font-size:14px;line-height:1.5}
  .header{padding:20px 24px;border-bottom:1px solid var(--border);display:flex;justify-content:space-between;align-items:center}
  .header h1{font-size:18px;font-weight:600;letter-spacing:-.02em}
  .header .live{display:flex;align-items:center;gap:6px;font-size:12px;color:var(--green)}
  .dot{width:8px;height:8px;border-radius:50%;background:var(--green);animation:pulse 2s infinite}
  @keyframes pulse{0%,100%{opacity:1}50%{opacity:.4}}
  .container{padding:20px 24px}
  .metrics{display:grid;grid-template-columns:repeat(4,1fr);gap:12px;margin-bottom:20px}
  .metric{background:var(--bg2);border:1px solid var(--border);border-radius:10px;padding:14px 16px}
  .metric-label{font-size:11px;color:var(--muted);text-transform:uppercase;letter-spacing:.06em;margin-bottom:6px}
  .metric-value{font-size:24px;font-weight:600}
  .metric-sub{font-size:11px;color:var(--muted);margin-top:3px}
  .green{color:var(--green)} .blue{color:var(--blue)} .amber{color:var(--amber)}
  .row2{display:grid;grid-template-columns:1fr 1fr;gap:16px;margin-bottom:16px}
  .row3{display:grid;grid-template-columns:1fr 1fr 1fr;gap:16px;margin-bottom:16px}
  .card{background:var(--bg2);border:1px solid var(--border);border-radius:10px;padding:16px}
  .card-title{font-size:11px;font-weight:600;text-transform:uppercase;letter-spacing:.06em;color:var(--muted);margin-bottom:14px}
  .chart-wrap{position:relative;height:180px}
  table{width:100%;border-collapse:collapse;font-size:13px}
  th{text-align:left;color:var(--muted);font-size:11px;text-transform:uppercase;letter-spacing:.05em;padding:6px 8px;border-bottom:1px solid var(--border);font-weight:500}
  td{padding:8px 8px;border-bottom:1px solid var(--border);color:var(--text)}
  tr:last-child td{border-bottom:none}
  tr:hover td{background:var(--bg3)}
  .badge{padding:2px 7px;border-radius:4px;font-size:11px;font-weight:500}
  .badge-strong{background:rgba(34,197,94,.15);color:#22c55e}
  .badge-medium{background:rgba(245,158,11,.15);color:#f59e0b}
  .badge-weak{background:rgba(239,68,68,.15);color:#ef4444}
  .bar-row{display:flex;align-items:center;gap:10px;margin-bottom:10px;font-size:13px}
  .bar-label{width:120px;color:var(--muted);white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
  .bar-track{flex:1;height:6px;background:var(--bg3);border-radius:3px;overflow:hidden}
  .bar-fill{height:100%;border-radius:3px;background:var(--blue)}
  .bar-val{width:60px;text-align:right;font-size:12px;color:var(--green)}
  .refresh-btn{background:var(--bg3);border:1px solid var(--border);color:var(--text);padding:6px 14px;border-radius:6px;cursor:pointer;font-size:13px}
  .refresh-btn:hover{border-color:var(--blue);color:var(--blue)}
  .usdt-badge{font-size:12px;padding:3px 8px;border-radius:4px;background:rgba(59,130,246,.15);color:var(--blue)}
</style>
</head>
<body>
<div class="header">
  <h1>Crypto Arb Scanner</h1>
  <div style="display:flex;gap:12px;align-items:center">
    <span id="usdt-badge" class="usdt-badge">USDT —</span>
    <div class="live"><div class="dot"></div><span id="last-update">—</span></div>
    <button class="refresh-btn" onclick="loadData()">Обновить</button>
  </div>
</div>

<div class="container">
  <div class="metrics">
    <div class="metric">
      <div class="metric-label">Всего сигналов</div>
      <div class="metric-value green" id="m-total">—</div>
      <div class="metric-sub" id="m-session">загрузка...</div>
    </div>
    <div class="metric">
      <div class="metric-label">Avg profit (norm)</div>
      <div class="metric-value blue" id="m-avg-profit">—</div>
      <div class="metric-sub" id="m-raw-spread">raw spread —</div>
    </div>
    <div class="metric">
      <div class="metric-label">Max profit</div>
      <div class="metric-value amber" id="m-max-profit">—</div>
      <div class="metric-sub">лучший сигнал</div>
    </div>
    <div class="metric">
      <div class="metric-label">Реальная прибыль</div>
      <div class="metric-value green" id="m-exp-usd">—</div>
      <div class="metric-sub" id="m-exp-sub">при $500/бирже</div>
    </div>
  </div>

  <div class="row3">
    <div class="card">
      <div class="card-title">По символам</div>
      <div class="chart-wrap"><canvas id="symChart"></canvas></div>
    </div>
    <div class="card">
      <div class="card-title">Profit distribution</div>
      <div class="chart-wrap"><canvas id="profitChart"></canvas></div>
    </div>
    <div class="card">
      <div class="card-title">Signal duration</div>
      <div class="chart-wrap"><canvas id="durChart"></canvas></div>
    </div>
  </div>

  <div class="row2">
    <div class="card">
      <div class="card-title">Топ пары</div>
      <div id="pairs-bars"></div>
    </div>
    <div class="card">
      <div class="card-title">Raw vs Normalized spread</div>
      <div class="chart-wrap" style="height:160px"><canvas id="spreadChart"></canvas></div>
    </div>
  </div>

  <div class="card">
    <div class="card-title">Последние сигналы</div>
    <table>
      <thead><tr>
        <th>Время</th><th>Пара</th><th>Символ</th>
        <th>Profit</th><th>Norm spread</th><th>Size</th>
        <th>Duration</th><th>Score</th><th>Status</th>
      </tr></thead>
      <tbody id="signals-table"></tbody>
    </table>
  </div>
</div>

<script>
let charts = {};

function fmt(n, dec=4) { return (+n).toFixed(dec); }
function fmtMs(ms) { return ms < 1000 ? ms+'ms' : (ms/1000).toFixed(1)+'s'; }
function fmtTime(ts) {
  const d = new Date(ts);
  return d.toLocaleTimeString('ru-RU');
}

function destroyChart(id) { if(charts[id]) { charts[id].destroy(); delete charts[id]; } }

function loadData() {
  fetch('/data')
    .then(r => r.json())
    .then(data => {
      if (!data) { document.getElementById('m-total').textContent = 'нет данных'; return; }
      renderData(data);
    })
    .catch(e => console.error(e));
}

function renderData(d) {
  const s = d.stats;
  document.getElementById('m-total').textContent = s.total || 0;
  // Показываем что это уникальные окна
  document.getElementById('m-session').textContent = 'уникальных окон';
  document.getElementById('m-avg-profit').textContent = fmt(s.avg_profit || 0) + '%';
  document.getElementById('m-raw-spread').textContent = 'raw ' + fmt(s.avg_raw || 0) + '%';
  document.getElementById('m-max-profit').textContent = fmt(s.max_profit || 0) + '%';
  const realProfit = s.real_profit_usd || 0;
  document.getElementById('m-exp-usd').textContent = '$' + fmt(realProfit, 2);
  document.getElementById('m-exp-sub').textContent = 'при $' + (d.capital || 500).toFixed(0) + '/бирже (из .env)';
  document.getElementById('last-update').textContent = new Date().toLocaleTimeString('ru-RU');

  // USDT rate из последнего сигнала
  if (d.opps.length) {
    const rate = d.opps[0].usdt_rate;
    const dev = ((rate - 1) * 100).toFixed(4);
    document.getElementById('usdt-badge').textContent = `USDT ${rate} (${dev > 0 ? '+' : ''}${dev}%)`;
  }

  // Symbols chart
  destroyChart('sym');
  charts['sym'] = new Chart(document.getElementById('symChart'), {
    type: 'bar',
    data: {
      labels: d.by_sym.map(r => r.symbol),
      datasets: [{ label: 'Сигналов', data: d.by_sym.map(r => r.cnt),
        backgroundColor: ['#22c55e','#3b82f6','#f59e0b','#a855f7','#ec4899'],
        borderRadius: 4, borderSkipped: false }]
    },
    options: { responsive:true, maintainAspectRatio:false, plugins:{legend:{display:false}},
      scales:{x:{grid:{display:false},ticks:{color:'#7b8099'}},y:{grid:{color:'rgba(255,255,255,0.05)'},ticks:{color:'#7b8099',stepSize:1}}} }
  });

  // Profit distribution
  const buckets = {'0.01-0.02':0,'0.02-0.04':0,'0.04-0.07':0,'>0.07':0};
  d.opps.forEach(o => {
    const p = o.profit_pct;
    if(p<0.02) buckets['0.01-0.02']++;
    else if(p<0.04) buckets['0.02-0.04']++;
    else if(p<0.07) buckets['0.04-0.07']++;
    else buckets['>0.07']++;
  });
  destroyChart('profit');
  charts['profit'] = new Chart(document.getElementById('profitChart'), {
    type: 'bar',
    data: { labels: Object.keys(buckets), datasets: [{ data: Object.values(buckets),
      backgroundColor:'#3b82f6', borderRadius:4, borderSkipped:false }] },
    options: { responsive:true, maintainAspectRatio:false, plugins:{legend:{display:false}},
      scales:{x:{grid:{display:false},ticks:{color:'#7b8099'}},y:{grid:{color:'rgba(255,255,255,0.05)'},ticks:{color:'#7b8099',stepSize:1}}} }
  });

  // Duration chart
  destroyChart('dur');
  charts['dur'] = new Chart(document.getElementById('durChart'), {
    type: 'doughnut',
    data: { labels: Object.keys(d.dur_buckets), datasets: [{ data: Object.values(d.dur_buckets),
      backgroundColor:['#ef4444','#f59e0b','#22c55e','#3b82f6'], borderWidth:0 }] },
    options: { responsive:true, maintainAspectRatio:false,
      plugins:{ legend:{ position:'bottom', labels:{color:'#7b8099',font:{size:11},boxWidth:10,padding:8} } } }
  });

  // Pairs bars
  const pairEl = document.getElementById('pairs-bars');
  pairEl.innerHTML = '';
  const maxCnt = Math.max(...d.by_pair.map(r=>r.cnt),1);
  d.by_pair.slice(0,6).forEach(r => {
    pairEl.innerHTML += `<div class="bar-row">
      <span class="bar-label">${r.pair} ${r.symbol}</span>
      <div class="bar-track"><div class="bar-fill" style="width:${r.cnt/maxCnt*100}%"></div></div>
      <span class="bar-val">+${fmt(r.max_profit)}%</span>
    </div>`;
  });

  // Spread comparison (last 20 snapshots)
  const sp = d.opps.slice(0,20).reverse();
  destroyChart('spread');
  charts['spread'] = new Chart(document.getElementById('spreadChart'), {
    type:'line',
    data:{
      labels: sp.map((_,i)=>i+1),
      datasets:[
        {label:'Raw %',data:sp.map(r=>+fmt(r.raw_spread_pct)),borderColor:'#ef4444',backgroundColor:'rgba(239,68,68,0.08)',tension:0.3,fill:true,pointRadius:2},
        {label:'Norm %',data:sp.map(r=>+fmt(r.normalized_spread_pct)),borderColor:'#22c55e',backgroundColor:'rgba(34,197,94,0.08)',tension:0.3,fill:true,pointRadius:2},
      ]
    },
    options:{responsive:true,maintainAspectRatio:false,
      plugins:{legend:{labels:{color:'#7b8099',font:{size:11},boxWidth:10}}},
      scales:{x:{display:false},y:{grid:{color:'rgba(255,255,255,0.05)'},ticks:{color:'#7b8099'}}}}
  });

  // Signals table
  const tbody = document.getElementById('signals-table');
  tbody.innerHTML = '';
  d.opps.slice(0,20).forEach(o => {
    const statusCls = o.signal_status === 'strong' ? 'strong' : o.signal_status === 'medium' ? 'medium' : 'weak';
    tbody.innerHTML += `<tr>
      <td style="color:#7b8099">${fmtTime(o.timestamp_ms)}</td>
      <td>${o.buy_exchange} → ${o.sell_exchange}</td>
      <td>${o.symbol}</td>
      <td style="color:#22c55e;font-weight:500">+${fmt(o.profit_pct)}%</td>
      <td>${fmt(o.normalized_spread_pct)}%</td>
      <td>$${fmt(o.max_size_usd, 0)}</td>
      <td>${fmtMs(o.signal_duration_ms)}</td>
      <td>${o.signal_score}/10</td>
      <td><span class="badge badge-${statusCls}">${o.signal_status}</span></td>
    </tr>`;
  });
}

// Автообновление каждые 10 сек
loadData();
setInterval(loadData, 10000);
</script>
</body>
</html>'''

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def do_GET(self):
        if self.path == '/data':
            data = get_data()
            body = json.dumps(data, default=str).encode()
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', len(body))
            self.end_headers()
            self.wfile.write(body)
        else:
            body = HTML.encode()
            self.send_response(200)
            self.send_header('Content-Type', 'text/html; charset=utf-8')
            self.send_header('Content-Length', len(body))
            self.end_headers()
            self.wfile.write(body)

if __name__ == '__main__':
    srv = http.server.HTTPServer(('', PORT), Handler)
    url = f'http://localhost:{PORT}'
    print(f'Dashboard: {url}')
    print(f'База данных: {DB}')
    print('Ctrl+C для остановки')
    threading.Timer(0.5, lambda: webbrowser.open(url)).start()
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        print('\nОстановлен')
