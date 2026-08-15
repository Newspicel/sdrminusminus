// Convenience aliases over the generated OpenAPI schema. These are the ONLY wire types —
// hand-writing a mirror of a Rust struct is forbidden (CLAUDE.md #1); regenerate instead.
import type { components, operations } from "../generated/schema";

export type StateSnapshot = components["schemas"]["StateSnapshot"];
export type StateScope = components["schemas"]["StateScope"];
export type DeviceSet = components["schemas"]["DeviceSet"];
export type DeviceInfo = components["schemas"]["DeviceInfo"];
export type DeviceSettings = components["schemas"]["DeviceSettings"];
export type Capabilities = components["schemas"]["Capabilities"];
export type DeviceProfile = components["schemas"]["DeviceProfile"];
export type StreamScope = components["schemas"]["StreamScope"];
export type StreamSettings = components["schemas"]["StreamSettings"];
export type Duplex = components["schemas"]["Duplex"];
export type GainStage = components["schemas"]["GainStage"];
export type ExtraSetting = components["schemas"]["ExtraSetting"];
export type DevicesResponse = components["schemas"]["DevicesResponse"];
export type ChannelInfo = components["schemas"]["ChannelInfo"];
export type ChannelLevel = components["schemas"]["ChannelLevel"];
export type OccupancyReport = components["schemas"]["OccupancyReport"];
export type OccupancyBucket = components["schemas"]["OccupancyBucket"];
export type ChannelSettings = components["schemas"]["ChannelSettings"];
export type ChannelParams = components["schemas"]["ChannelParams"];
export type ChannelDescriptor = components["schemas"]["ChannelDescriptor"];
export type ChannelTypesResponse = components["schemas"]["ChannelTypesResponse"];
export type PresetInfo = components["schemas"]["PresetInfo"];
export type Bookmark = components["schemas"]["Bookmark"];
export type CreateBookmarkRequest = components["schemas"]["CreateBookmarkRequest"];
export type RecordAction = components["schemas"]["RecordAction"];
export type RecordingStatus = components["schemas"]["RecordingStatus"];
export type PlaybackStatus = components["schemas"]["PlaybackStatus"];
export type PlaybackAction = components["schemas"]["PlaybackAction"];
export type RecordingInfo = components["schemas"]["RecordingInfo"];
export type RecordingsResponse = components["schemas"]["RecordingsResponse"];
export type VoiceCall = components["schemas"]["VoiceCall"];
export type DvTrunkProtocol = components["schemas"]["DvTrunkProtocol"];
export type TrunkSystemStatus = components["schemas"]["TrunkSystemStatus"];
export type TrunkFollower = components["schemas"]["TrunkFollower"];
export type TrunkProblem = components["schemas"]["TrunkProblem"];
export type VoiceCallsResponse = components["schemas"]["VoiceCallsResponse"];
export type RecordingFormat = components["schemas"]["RecordingFormat"];
export type DecodedRecord = components["schemas"]["DecodedRecord"];
export type DecoderEvent = components["schemas"]["DecoderEvent"];
export type DecoderLogEntry = components["schemas"]["DecoderLogEntry"];
export type DecoderLogResponse = components["schemas"]["DecoderLogResponse"];
export type DeletedCount = components["schemas"]["DeletedCount"];
export type ExportFormat = components["schemas"]["ExportFormat"];
export type AdsbMessage = components["schemas"]["AdsbMessage"];
export type AisMessage = components["schemas"]["AisMessage"];
export type AprsPacket = components["schemas"]["AprsPacket"];
export type PocsagMessage = components["schemas"]["PocsagMessage"];
export type RdsUpdate = components["schemas"]["RdsUpdate"];
export type RttyText = components["schemas"]["RttyText"];
export type MorseText = components["schemas"]["MorseText"];
export type DvFrame = components["schemas"]["DvFrame"];
export type IdentReport = components["schemas"]["IdentReport"];
export type ProtocolMatch = components["schemas"]["ProtocolMatch"];
export type Modulation = components["schemas"]["Modulation"];
export type ServerEvent = components["schemas"]["ServerEvent"];
export type StreamKind = components["schemas"]["StreamKind"];
export type ClientCommand = components["schemas"]["ClientCommand"];
export type ApiError = components["schemas"]["ApiError"];
export type AuthInfo = components["schemas"]["AuthInfo"];
export type ClientsResponse = components["schemas"]["ClientsResponse"];
export type TemplateInfo = components["schemas"]["TemplateInfo"];
export type TemplatesResponse = components["schemas"]["TemplatesResponse"];
export type ScanSettings = components["schemas"]["ScanSettings"];
export type ScanRange = components["schemas"]["ScanRange"];
export type ScannerStatus = components["schemas"]["ScannerStatus"];
export type ScanState = components["schemas"]["ScanState"];
export type WorkspaceInfo = components["schemas"]["WorkspaceInfo"];
export type WorkspacesResponse = components["schemas"]["WorkspacesResponse"];
export type WorkspaceDetail = components["schemas"]["WorkspaceDetail"];
export type WorkspaceSnapshot = components["schemas"]["WorkspaceSnapshot"];
export type WorkspaceSettings = components["schemas"]["WorkspaceSettings"];
export type PatchGraph = components["schemas"]["PatchGraph"];
export type PatchNode = components["schemas"]["PatchNode"];
export type Position = components["schemas"]["Position"];
export type NodeBody = components["schemas"]["NodeBody"];
export type NodeKind = NodeBody["kind"];
export type DmrTrunkProtocol = components["schemas"]["DmrTrunkProtocol"];
export type NodeCategory = components["schemas"]["NodeCategory"];
export type PatchEdge = components["schemas"]["PatchEdge"];
export type PortRef = components["schemas"]["PortRef"];
export type PortSpec = components["schemas"]["PortSpec"];
export type PortType = components["schemas"]["PortType"];
export type PositionFix = components["schemas"]["PositionFix"];
export type PositionSource = components["schemas"]["PositionSource"];
export type NmeaDeviceInfo = components["schemas"]["NmeaDeviceInfo"];
export type NmeaDevicesResponse = components["schemas"]["NmeaDevicesResponse"];
export type PortDirection = components["schemas"]["PortDirection"];
export type PortCondition = components["schemas"]["PortCondition"];
export type PatchCatalog = components["schemas"]["PatchCatalog"];
export type NodeTypeInfo = components["schemas"]["NodeTypeInfo"];
export type DeviceRef = components["schemas"]["DeviceRef"];
export type RackLayout = components["schemas"]["RackLayout"];
export type RackSlot = components["schemas"]["RackSlot"];
export type PatchApplyReport = components["schemas"]["PatchApplyReport"];
export type PatchBinding = components["schemas"]["PatchBinding"];
export type BandPlan = components["schemas"]["BandPlan"];
export type BandLane = components["schemas"]["BandLane"];
export type BandBlock = components["schemas"]["BandBlock"];
export type BandAllocation = components["schemas"]["BandAllocation"];
export type BandLayerInfo = components["schemas"]["BandLayerInfo"];
export type BandRegion = components["schemas"]["BandRegion"];
export type BandRegionsResponse = components["schemas"]["BandRegionsResponse"];
export type BandService = components["schemas"]["BandService"];
export type AboutResponse = components["schemas"]["AboutResponse"];
export type Attribution = components["schemas"]["Attribution"];
export type ComponentSource = components["schemas"]["ComponentSource"];
export type LicenseTextResponse = components["schemas"]["LicenseTextResponse"];
export type DoctorReport = components["schemas"]["DoctorReport"];
export type DoctorCheck = components["schemas"]["DoctorCheck"];
export type CheckStatus = components["schemas"]["CheckStatus"];

// `DecoderLogQuery` is flattened into the operation's query parameters by utoipa, so it has no
// `components.schemas` entry — deriving it from `operations` keeps it generated either way.
export type DecoderLogFilter = NonNullable<operations["list_decoder_log"]["parameters"]["query"]>;

// Discriminant and per-variant projections of the generated `DecoderEvent` union, so a panel or
// store can be typed on one decoder without re-declaring its payload.
export type DecoderKind = DecoderEvent["kind"];
export type DecoderEventOf<K extends DecoderKind> = Extract<DecoderEvent, { kind: K }>;
export type DecodedRecordOf<K extends DecoderKind> = Omit<DecodedRecord, "event"> & {
  event: DecoderEventOf<K>;
};

// The node body narrowed to one kind, so a face is typed on the payload it renders without
// re-declaring it.
export type NodeBodyOf<K extends NodeKind> = Extract<NodeBody, { kind: K }>;
export type PatchNodeOf<K extends NodeKind> = Omit<PatchNode, keyof NodeBody> & NodeBodyOf<K>;
