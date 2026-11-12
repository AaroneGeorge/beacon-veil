{
  description = "beacon-veil bridge_ui: QML mini-app skeleton (instance status; static deposit, claim, recover)";

  inputs = {
    # Same builder revision as attest_follower_module and beacon_api_module v0.2.0.
    logos-module-builder.url = "github:logos-co/logos-module-builder/a57dc7fd1fe92d3ac7b545c5559b9c581576f2c9";
  };

  outputs = inputs@{ logos-module-builder, ... }:
    logos-module-builder.lib.mkLogosQmlModule {
      src = ./.;
      configFile = ./metadata.json;
      flakeInputs = inputs;
    };
}
