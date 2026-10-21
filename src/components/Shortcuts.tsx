const GROUPS: { title: string; keys: [string, string][] }[] = [
  {
    title: "Anywhere",
    keys: [
      ["⌘K  /  /", "Ask or act on anything"],
      ["1 – 6", "Today, Inbox, Time, Tickets, Meetings, Settings"],
      ["r", "Sync with GitHub now"],
      ["s  /  t", "Day view (time spent)  /  Timesheet"],
      ["?", "This sheet"],
      ["⌥⌘K", "Open Dev Inbox from any app"],
    ],
  },
  {
    title: "In a list",
    keys: [
      ["j / k  or  ↓ / ↑", "Move"],
      ["click", "Show details and actions"],
      ["↵  or  o", "Open on GitHub (or expand a group)"],
      ["Esc", "Clear selection"],
      ["right-click  or  .", "Quick menu"],
    ],
  },
  {
    title: "On the selected item",
    keys: [
      ["e", "Done"],
      ["l", "Come back later…"],
      ["x", "Back to automatic"],
      ["R", "Reply"],
      ["a", "Approve…"],
      ["m", "Merge…"],
      ["c", "Check out & open in editor"],
    ],
  },
];

export function Shortcuts({ onClose }: { onClose: () => void }) {
  return (
    <div className="overlay" onMouseDown={onClose}>
      <div
        className="dialog wide shortcuts"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => e.key === "Escape" && onClose()}
        tabIndex={-1}
        ref={(el) => el?.focus()}
      >
        <div className="dialog-head">
          <span className="strong">Keyboard shortcuts</span>
        </div>
        <div className="shortcut-grid">
          {GROUPS.map((g) => (
            <section key={g.title}>
              <div className="group-label">{g.title}</div>
              {g.keys.map(([k, v]) => (
                <div key={k} className="shortcut-row">
                  <span>{v}</span>
                  <kbd>{k}</kbd>
                </div>
              ))}
            </section>
          ))}
        </div>
        <div className="dialog-foot">
          <span className="muted small">Every action is also a button in the details pane.</span>
          <button onClick={onClose}>Close</button>
        </div>
      </div>
    </div>
  );
}
