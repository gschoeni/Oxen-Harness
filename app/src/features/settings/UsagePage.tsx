import { ChevronLeft, ChevronRight } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { dailyUsage, modelUsageBreakdown } from "../../lib/ipc";
import { compactTokens, formatUsd } from "../../lib/format";
import type { DailyUsageRow, UsageBreakdown } from "../../lib/types";

const EMPTY: UsageBreakdown = {
  rows: [],
  total_cost_usd: 0,
  prompt_tokens: 0,
  completion_tokens: 0,
  has_unpriced_usage: false,
};

export function UsagePage() {
  const currentYear = new Date().getFullYear();
  const [year, setYear] = useState(currentYear);
  const [days, setDays] = useState<DailyUsageRow[]>([]);
  const [selectedDate, setSelectedDate] = useState<string | null>(null);
  const [usage, setUsage] = useState<UsageBreakdown | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    dailyUsage(year)
      .then((next) => active && setDays(next))
      .catch((e) => active && setError(String(e)));
    return () => { active = false; };
  }, [year]);

  useEffect(() => {
    let active = true;
    setUsage(null);
    modelUsageBreakdown(selectedDate)
      .then((next) => active && setUsage(next))
      .catch((e) => active && setError(String(e)));
    return () => { active = false; };
  }, [selectedDate]);

  const report = usage ?? EMPTY;
  const reportLabel = selectedDate
    ? new Date(`${selectedDate}T00:00:00`).toLocaleDateString(undefined, {
        month: "long", day: "numeric", year: "numeric",
      })
    : "All time";
  const maxModelTokens = Math.max(
    1,
    ...report.rows.map((row) => row.prompt_tokens + row.completion_tokens),
  );

  function changeYear(next: number) {
    setYear(next);
    if (selectedDate && Number(selectedDate.slice(0, 4)) !== next) setSelectedDate(null);
  }

  return (
    <div className="settings-page usage-page">
      <section className="usage-hero">
        <div>
          <div className="settings-label">Model activity</div>
          <h3>Usage over time</h3>
          <p>Every model call, measured from provider-reported tokens when available.</p>
        </div>
        <div className="usage-year-nav" aria-label="Activity year">
          <button aria-label="Previous year" onClick={() => changeYear(year - 1)}>
            <ChevronLeft size={15} />
          </button>
          <span>{year}</span>
          <button
            aria-label="Next year"
            disabled={year >= currentYear}
            onClick={() => changeYear(year + 1)}
          >
            <ChevronRight size={15} />
          </button>
        </div>
      </section>

      <section className="usage-calendar-card" aria-label={`${year} token activity`}>
        <ContributionGrid
          year={year}
          days={days}
          selectedDate={selectedDate}
          onSelect={setSelectedDate}
        />
        <div className="usage-calendar-footer">
          <span>{days.length} active {days.length === 1 ? "day" : "days"}</span>
          <div className="usage-legend" aria-label="Less to more token usage">
            <span>Less</span>
            {[0, 1, 2, 3, 4].map((level) => <i key={level} data-level={level} />)}
            <span>More</span>
          </div>
        </div>
      </section>

      <section className="usage-report-head">
        <div>
          <div className="settings-label">Report</div>
          <h3>{reportLabel}</h3>
        </div>
        {selectedDate && (
          <button className="usage-all-time" onClick={() => setSelectedDate(null)}>
            View all time
          </button>
        )}
      </section>

      <section className="usage-stats" aria-busy={usage === null}>
        <UsageStat label="Input tokens" value={usage ? compactTokens(report.prompt_tokens) : "—"} />
        <UsageStat label="Output tokens" value={usage ? compactTokens(report.completion_tokens) : "—"} />
        <UsageStat
          label="Estimated spend"
          value={usage && report.total_cost_usd !== null ? formatUsd(report.total_cost_usd) : "—"}
          note={report.has_unpriced_usage ? "plus unpriced usage" : undefined}
        />
      </section>

      <section className="usage-breakdown">
        <div className="usage-breakdown-head">
          <span>Model</span><span>Tokens</span><span>Estimated cost</span>
        </div>
        {usage && report.rows.length === 0 ? (
          <div className="usage-empty">No model activity for this period.</div>
        ) : report.rows.map((row) => {
          const total = row.prompt_tokens + row.completion_tokens;
          const width = Math.max(2, (total / maxModelTokens) * 100);
          return (
            <div className="usage-model-row" key={`${row.model}:${row.source}`}>
              <div className="usage-model-cell">
                <div>
                  <strong title={row.model}>{row.model}</strong>
                  {row.source !== "oxen_cloud" && <small>local / custom</small>}
                </div>
                <div className="usage-model-bar"><i style={{ width: `${width}%` }} /></div>
              </div>
              <div className="usage-token-cell">
                <strong>{compactTokens(total)}</strong>
                <small>{compactTokens(row.prompt_tokens)} in · {compactTokens(row.completion_tokens)} out</small>
              </div>
              <strong className="usage-price">
                {row.cost_usd === null ? "—" : formatUsd(row.cost_usd)}
              </strong>
            </div>
          );
        })}
      </section>

      <p className="usage-method-note">
        Spend is an estimate using rates advertised by the connected Oxen-compatible endpoint.
        Local, custom, and unlisted models without published rates remain unpriced instead of being
        counted as free. Daily and per-model tracking begins with this version; older transcripts
        remain represented only in the all-time token estimate.
      </p>
      {error && <span className="save-status err">{error}</span>}
    </div>
  );
}

function UsageStat({ label, value, note }: { label: string; value: string; note?: string }) {
  return (
    <div className="usage-stat">
      <span>{label}</span>
      <strong>{value}</strong>
      {note && <small>{note}</small>}
    </div>
  );
}

function ContributionGrid({
  year,
  days,
  selectedDate,
  onSelect,
}: {
  year: number;
  days: DailyUsageRow[];
  selectedDate: string | null;
  onSelect: (date: string) => void;
}) {
  const activity = useMemo(
    () => new Map(days.map((day) => [day.date, day.prompt_tokens + day.completion_tokens])),
    [days],
  );
  const max = Math.max(0, ...activity.values());
  const { cells, months } = useMemo(() => calendarCells(year), [year]);

  return (
    <div className="usage-grid-scroll">
      <div className="usage-grid-shell">
        <div className="usage-months">
          {months.map((month) => (
            <span key={month.label} style={{ gridColumn: month.column }}>{month.label}</span>
          ))}
        </div>
        <div className="usage-weekdays" aria-hidden="true"><span>Mon</span><span>Wed</span><span>Fri</span></div>
        <div className="usage-grid">
          {cells.map((date, index) => {
            if (!date) return <i className="usage-day-spacer" key={`empty-${index}`} />;
            const tokens = activity.get(date) ?? 0;
            const level = tokens === 0 || max === 0 ? 0 : Math.max(1, Math.ceil(Math.sqrt(tokens / max) * 4));
            const prettyDate = new Date(`${date}T00:00:00`).toLocaleDateString(undefined, {
              weekday: "short", month: "short", day: "numeric", year: "numeric",
            });
            const tooltip = `${prettyDate} · ${tokens.toLocaleString()} tokens`;
            return (
              <button
                className="usage-day"
                data-level={level}
                data-tooltip={tooltip}
                data-selected={selectedDate === date || undefined}
                title={tooltip}
                aria-label={tooltip}
                aria-pressed={selectedDate === date}
                key={date}
                onClick={() => onSelect(date)}
              />
            );
          })}
        </div>
      </div>
    </div>
  );
}

function calendarCells(year: number) {
  const first = new Date(year, 0, 1);
  const last = new Date(year, 11, 31);
  const start = new Date(first);
  start.setDate(first.getDate() - first.getDay());
  const cellCount = Math.ceil((calendarDayDistance(start, last) + 1) / 7) * 7;
  const cells: Array<string | null> = [];
  for (let i = 0; i < cellCount; i++) {
    const day = new Date(start);
    day.setDate(start.getDate() + i);
    cells.push(day.getFullYear() === year ? localDateKey(day) : null);
  }
  const months = Array.from({ length: 12 }, (_, month) => {
    const date = new Date(year, month, 1);
    const column = Math.floor(calendarDayDistance(start, date) / 7) + 1;
    return { label: date.toLocaleDateString(undefined, { month: "short" }), column };
  }).filter((month, index, all) => index === 0 || month.column !== all[index - 1].column);
  return { cells, months };
}

function calendarDayDistance(from: Date, to: Date): number {
  const utc = (date: Date) => Date.UTC(date.getFullYear(), date.getMonth(), date.getDate());
  return Math.round((utc(to) - utc(from)) / 86_400_000);
}

function localDateKey(date: Date): string {
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${date.getFullYear()}-${month}-${day}`;
}
