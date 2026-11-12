{
  description = "beacon-veil attest_follower_module: Logos core module that supervises attest-followerd";

  inputs = {
    # Same builder revision as the beacon_api_module v0.2.0 we run beside (its flake.lock), so both plugins link one
    # Qt / logos-protocol closure.
    logos-module-builder.url = "github:logos-co/logos-module-builder/a57dc7fd1fe92d3ac7b545c5559b9c581576f2c9";
    # Declared dependency (metadata.json): the builder reads its published LIDL contract from this input.
    # v0.2.0 commit, the same one installed beside this module.
    beacon_api_module.url = "github:logos-co/logos-evm-beacon-api-module/aca138e5c211a370654700f4f8f2779ea5081c86";
    beacon_api_module.inputs.logos-module-builder.follows = "logos-module-builder";
  };

  outputs = inputs@{ logos-module-builder, ... }:
    logos-module-builder.lib.mkLogosModule {
      src = ./.;
      configFile = ./metadata.json;
      flakeInputs = inputs;
    };
}
