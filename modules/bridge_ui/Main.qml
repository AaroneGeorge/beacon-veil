// beacon-veil bridge mini-app: skeleton (RFP-022 U2, U5, U6).
// Live: the light-client instance status, from attest_follower_module.status().
// Static: deposit, claim and recover screens. The private path is preselected everywhere (U5, U6). The actions are
// done today with tools/bridge_cli; these screens show the flow and the defaults, and send nothing.
import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15

Item {
    id: root
    width: 520
    height: 460

    property var status: ({})
    property string statusError: ""

    function refresh() {
        if (typeof logos === "undefined") {
            statusError = "Logos host bridge not available"
            return
        }
        try {
            var r = logos.callModule("attest_follower_module", "status", [])
            status = (typeof r === "string") ? JSON.parse(r) : r
            statusError = ""
        } catch (e) {
            statusError = "status failed: " + e
        }
    }

    function field(path) {
        var v = status
        var parts = path.split(".")
        for (var i = 0; i < parts.length; i++) {
            if (v === undefined || v === null) return "-"
            v = v[parts[i]]
        }
        return (v === undefined || v === null) ? "-" : String(v)
    }

    Timer { interval: 15000; running: true; repeat: true; triggeredOnStart: true; onTriggered: root.refresh() }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 12
        spacing: 8

        TabBar {
            id: tabs
            Layout.fillWidth: true
            TabButton { text: "Instance" }
            TabButton { text: "Deposit" }
            TabButton { text: "Claim" }
            TabButton { text: "Recover" }
        }

        StackLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            currentIndex: tabs.currentIndex

            // 0. Instance status (live).
            GridLayout {
                columns: 2
                columnSpacing: 12
                Label { text: "Follower running" } Label { text: root.field("running") }
                Label { text: "Follower pid" } Label { text: root.field("pid") }
                Label { text: "Head slot (last line)" } Label { text: root.field("last_line.head_slot") }
                Label { text: "Sync period" } Label { text: root.field("last_line.period") }
                Label { text: "Next committee known" } Label { text: root.field("last_line.next_committee_known") }
                Label { text: "Last event" } Label { text: root.field("last_line.event") + " at " + root.field("last_line.ts") }
                Label { text: "Last applied tx" } Label { text: root.field("cache.last_tx"); elide: Text.ElideMiddle; Layout.maximumWidth: 300 }
                Label { text: "Beacon source" } Label { text: root.field("last_line.beacon_source") }
                Label { text: root.statusError; color: "darkred"; Layout.columnSpan: 2; visible: root.statusError !== "" }
                Button { text: "Refresh"; onClicked: root.refresh() }
            }

            // 1. Deposit (static).
            ColumnLayout {
                Label { text: "Deposit on Ethereum (Sepolia vault). Fixed denominations; nothing on Ethereum names the destination." ; wrapMode: Text.WordWrap; Layout.fillWidth: true }
                RowLayout { Label { text: "Token" } ComboBox { model: ["ETH", "Test ERC-20"] } }
                RowLayout { Label { text: "Denomination" } ComboBox { model: ["0.01", "0.1", "1"] } }
                Label { text: "Destination on LEZ" }
                RadioButton { text: "Private account (recommended)"; checked: true }
                RadioButton { text: "Public account" }
                Label { text: "The commitment binds the destination; the secret derives from your LEZ wallet seed."; wrapMode: Text.WordWrap; Layout.fillWidth: true; font.italic: true }
                Button { text: "Deposit (skeleton: use bridge_cli deposit)"; enabled: false }
            }

            // 2. Claim (static).
            ColumnLayout {
                Label { text: "Claim on LEZ: one transaction. Attest the vault root through the light-client instance, then prove the deposit in zero knowledge."; wrapMode: Text.WordWrap; Layout.fillWidth: true }
                Label { text: "Mint to" }
                RadioButton { text: "Private account (recommended)"; checked: true }
                RadioButton { text: "Public account (the claim is still zero-knowledge)" }
                Label { text: "Reveals only nullifier, token and denomination. A second claim of one deposit is refused."; wrapMode: Text.WordWrap; Layout.fillWidth: true; font.italic: true }
                Button { text: "Claim (skeleton: use bridge_cli claim)"; enabled: false }
            }

            // 3. Recover (static).
            ColumnLayout {
                Label { text: "Recover from your existing LEZ wallet seed: scan the vault's deposits for commitments derived from your seed, and list unclaimed ones."; wrapMode: Text.WordWrap; Layout.fillWidth: true }
                CheckBox { text: "Claim found deposits to a private account"; checked: true }
                Button { text: "Scan (skeleton: use bridge_cli recover)"; enabled: false }
            }
        }
    }
}
