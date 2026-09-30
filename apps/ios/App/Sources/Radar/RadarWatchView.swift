import SdrmmCore
import SwiftUI

struct RadarWatchView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let radar = model.radar
        List {
            Section {
                RadarPicture()
                    .listRowInsets(EdgeInsets(top: 8, leading: 8, bottom: 8, trailing: 8))
                RadarChips()
            }
            Section {
                let rows = radar.rows
                if rows.isEmpty {
                    Text("No tracks")
                        .foregroundStyle(.secondary)
                        .accessibilityIdentifier(A11y.radarEmpty)
                } else {
                    ForEach(rows) { row in
                        RadarTrackRow(row: row)
                    }
                }
            } header: {
                Text("Tracks").textCase(nil)
            } footer: {
                Text(RadarText.echoes(radar.view?.echoes ?? 0))
                    .monospacedDigit()
                    .accessibilityIdentifier(A11y.radarEchoes)
            }
        }
    }
}

private struct RadarPicture: View {
    private static let axisWidth: CGFloat = 52
    private static let height: CGFloat = 200
    @Environment(AppModel.self) private var model

    var body: some View {
        let radar = model.radar
        VStack(spacing: 4) {
            HStack(alignment: .top, spacing: 4) {
                VStack {
                    Text(radar.axes?.dopplerTop ?? "")
                    Spacer()
                    Text(radar.axes?.dopplerBottom ?? "")
                }
                .font(.caption2.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: Self.axisWidth, height: Self.height, alignment: .trailing)
                picture(radar)
            }
            HStack {
                Text("0 km")
                Spacer()
                Text(radar.axes?.rangeEnd ?? "")
            }
            .font(.caption2.monospacedDigit())
            .foregroundStyle(.secondary)
            .padding(.leading, Self.axisWidth + 4)
        }
    }

    @ViewBuilder private func picture(_ radar: RadarModel) -> some View {
        if let image = radar.image {
            Image(decorative: image, scale: 1)
                .resizable()
                .interpolation(.none)
                .frame(maxWidth: .infinity)
                .frame(height: Self.height)
                .accessibilityElement()
                .accessibilityLabel("Range Doppler map")
                .accessibilityIdentifier(A11y.radarImage)
        } else {
            ZStack {
                RoundedRectangle(cornerRadius: 6).fill(.quaternary)
                if radar.imageFailed {
                    Text("No image")
                        .foregroundStyle(Palette.danger)
                        .accessibilityIdentifier(A11y.radarNoImage)
                } else {
                    ProgressView()
                }
            }
            .frame(maxWidth: .infinity)
            .frame(height: Self.height)
        }
    }
}

private struct RadarChips: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let view = model.radar.view
        let problems = view?.problems ?? []
        if view?.stale == true || !problems.isEmpty {
            HStack {
                if view?.stale == true {
                    Chip(text: "Stale", color: Palette.warn)
                        .accessibilityIdentifier(A11y.radarStale)
                }
                ForEach(problems, id: \.self) { problem in
                    Chip(text: problem, color: Palette.danger)
                        .accessibilityIdentifier(A11y.radarProblem)
                }
            }
        }
    }
}

private struct RadarTrackRow: View {
    let row: RadarRow

    var body: some View {
        HStack(spacing: 12) {
            Text(row.name).bold()
            Text(row.range)
            Text(row.doppler)
            Spacer()
            Text(row.motion)
                .foregroundStyle(row.closing ? Palette.warn : .secondary)
            if let bearing = row.bearing {
                Text(bearing)
            }
        }
        .font(.callout.monospacedDigit())
        .opacity(row.coasting ? 0.5 : 1)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier(A11y.radarTrack(row.id))
    }
}
