import { useState } from "react";
import { TABLE_CELL, TABLE_HEAD } from "../../components/controls";
import { Segmented } from "../../components/Segmented";
import type { Codeplug, CodeplugChannel, CodeplugContact } from "../../lib/types";
import { channelDetail, channelKind, formatMhz, formatShift, lists, zoneChannels } from "./cps";

type Section = "channels" | "zones" | "contacts" | "groups" | "scans" | "ids";

const SECTIONS = [
  { value: "channels", label: "Channels" },
  { value: "zones", label: "Zones" },
  { value: "contacts", label: "Contacts" },
  { value: "groups", label: "Group lists" },
  { value: "scans", label: "Scan lists" },
  { value: "ids", label: "Radio IDs" },
] as const;

export function CodeplugView({ codeplug }: { codeplug: Codeplug }) {
  const [section, setSection] = useState<Section>("channels");
  const parts = lists(codeplug);
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2">
      <Segmented
        label="Codeplug section"
        value={section}
        options={SECTIONS.map((entry) => ({ ...entry }))}
        onChange={setSection}
      />
      <div className="min-h-0 flex-1 overflow-auto rounded-[3px] border border-line">
        {section === "channels" && <Channels channels={parts.channels} />}
        {section === "zones" && (
          <Members
            head={["Zone", "Channels"]}
            rows={parts.zones.map((zone) => [zone.name, zoneChannels(zone)])}
          />
        )}
        {section === "contacts" && <Contacts contacts={parts.contacts} />}
        {section === "groups" && (
          <Members
            head={["Group list", "Contacts"]}
            rows={parts.groupLists.map((list) => [list.name, list.contacts ?? []])}
          />
        )}
        {section === "scans" && (
          <Members
            head={["Scan list", "Channels"]}
            rows={parts.scanLists.map((list) => [list.name, list.channels ?? []])}
          />
        )}
        {section === "ids" && (
          <table className="w-full border-collapse">
            <thead className="sticky top-0 bg-panel-3">
              <tr>
                <th className={TABLE_HEAD}>Name</th>
                <th className={TABLE_HEAD}>DMR ID</th>
              </tr>
            </thead>
            <tbody>
              {parts.radioIds.map((id) => (
                <tr key={id.name} className="border-t border-line">
                  <td className={TABLE_CELL}>{id.name}</td>
                  <td className={TABLE_CELL}>{id.number}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}

function Channels({ channels }: { channels: CodeplugChannel[] }) {
  return (
    <table className="w-full border-collapse">
      <thead className="sticky top-0 bg-panel-3">
        <tr>
          <th className={TABLE_HEAD}>#</th>
          <th className={TABLE_HEAD}>Name</th>
          <th className={TABLE_HEAD}>RX MHz</th>
          <th className={TABLE_HEAD}>Shift</th>
          <th className={TABLE_HEAD}>Mode</th>
          <th className={TABLE_HEAD}>Power</th>
          <th className={TABLE_HEAD}>Detail</th>
          <th className={TABLE_HEAD}>Scan list</th>
        </tr>
      </thead>
      <tbody>
        {channels.map((channel, index) => (
          <tr key={`${channel.name}-${index}`} className="border-t border-line">
            <td className={`${TABLE_CELL} text-ink-faint`}>{index + 1}</td>
            <td className={`${TABLE_CELL} text-ink`}>{channel.name}</td>
            <td className={TABLE_CELL}>{formatMhz(channel.rx_hz)}</td>
            <td className={`${TABLE_CELL} text-ink-dim`}>{formatShift(channel)}</td>
            <td className={TABLE_CELL}>{channelKind(channel).toUpperCase()}</td>
            <td className={`${TABLE_CELL} text-ink-dim`}>{channel.power ?? "mid"}</td>
            <td className={`${TABLE_CELL} text-ink-dim`}>{channelDetail(channel)}</td>
            <td className={`${TABLE_CELL} text-ink-dim`}>{channel.scan_list ?? "—"}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function Contacts({ contacts }: { contacts: CodeplugContact[] }) {
  return (
    <table className="w-full border-collapse">
      <thead className="sticky top-0 bg-panel-3">
        <tr>
          <th className={TABLE_HEAD}>Name</th>
          <th className={TABLE_HEAD}>Kind</th>
          <th className={TABLE_HEAD}>Number</th>
        </tr>
      </thead>
      <tbody>
        {contacts.map((contact) => (
          <tr key={contact.name} className="border-t border-line">
            <td className={`${TABLE_CELL} text-ink`}>{contact.name}</td>
            <td className={`${TABLE_CELL} text-ink-dim`}>{contact.kind ?? "group"}</td>
            <td className={TABLE_CELL}>{contact.number}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function Members({ head, rows }: { head: [string, string]; rows: [string, string[]][] }) {
  return (
    <table className="w-full border-collapse">
      <thead className="sticky top-0 bg-panel-3">
        <tr>
          <th className={TABLE_HEAD}>{head[0]}</th>
          <th className={TABLE_HEAD}>{head[1]}</th>
        </tr>
      </thead>
      <tbody>
        {rows.map(([name, members]) => (
          <tr key={name} className="border-t border-line">
            <td className={`${TABLE_CELL} whitespace-nowrap text-ink`}>{name}</td>
            <td className={`${TABLE_CELL} text-ink-dim`} title={members.join(", ")}>
              {members.length === 0 ? "—" : `${members.length} · ${members.join(", ")}`}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
