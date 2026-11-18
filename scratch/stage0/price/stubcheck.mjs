import fs from 'fs'
const src = fs.readFileSync(process.argv[2], 'utf8').replace(/^export const meta/m, 'const meta')
const labels = []
const agent = async (prompt, opts) => { labels.push((opts && opts.label) + ' ' + prompt.length); const sc = opts && opts.schema; const o = {}; if (sc && sc.properties) for (const k of Object.keys(sc.properties)) { const t = sc.properties[k].type; o[k] = t === 'array' ? [] : t === 'boolean' ? true : t === 'string' ? (k === 'verdict' ? 'pass' : 'x') : t === 'object' ? {} : 1 } ; if ('committed' in o) o.committed = true; if ('land' in o) o.land = true; return o }
const parallel = async ts => Promise.all(ts.map(t => t().catch(() => null)))
const pipeline = async (items, ...stages) => { const out = []; for (const [i, it] of items.entries()) { let r = it; for (const st of stages) r = await st(r, it, i); out.push(r) } return out }
const log = m => {}
const phase = t => {}
const args = { base: 'deadbeef' }
const budget = { total: null, spent: () => 0, remaining: () => Infinity }
const fn = new Function('agent', 'parallel', 'pipeline', 'log', 'phase', 'args', 'budget', 'return (async () => {' + src + '\n})()')
fn(agent, parallel, pipeline, log, phase, args, budget).then(r => { console.log(labels.join('\n')); console.log('ran ok') }).catch(e => { console.log(labels.join('\n')); console.log('ERR', e) })
