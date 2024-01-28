use bbl_usd::{cpp, sdf, usd};

pub struct LocalLayers {
    root: sdf::LayerRefPtr,
    current_sublayer: sdf::LayerRefPtr,
    private: sdf::LayerRefPtr,
    sublayer_index: u8,
}

impl LocalLayers {
    pub fn new(root: &sdf::LayerHandle) -> Self {
        let local_root = sdf::Layer::create_anonymous(".usdc");
        root.insert_sub_layer_path(local_root.get_identifier(), 0);

        let current_sublayer = sdf::Layer::create_anonymous(".usdc");

        local_root.insert_sub_layer_path(current_sublayer.get_identifier(), 0);

        let private = sdf::Layer::create_anonymous(".usdc");

        root.insert_sub_layer_path(private.get_identifier(), 0);

        Self {
            root: local_root,
            current_sublayer,
            private,
            sublayer_index: 0,
        }
    }

    pub fn set_private_edit_target(&mut self, stage: &usd::StageRefPtr) {
        let edit_target = usd::EditTarget::new_from_layer_ref_ptr(&self.private);
        stage.set_edit_target(&edit_target);
    }

    pub fn set_public_edit_target(&mut self, stage: &usd::StageRefPtr) {
        let edit_target = usd::EditTarget::new_from_layer_ref_ptr(&self.current_sublayer);
        stage.set_edit_target(&edit_target);
    }

    pub fn add_new_sublayer(&mut self) {
        let new_sublayer = sdf::Layer::create_anonymous(".usdc");

        self.root
            .insert_sub_layer_path(new_sublayer.get_identifier(), 0);

        self.current_sublayer = new_sublayer;
        self.sublayer_index += 1;
    }

    pub fn export(&self) -> (u8, cpp::String) {
        let state = self.current_sublayer.export_to_string().unwrap();
        (self.sublayer_index, state)
    }
}

pub fn update_remote_sublayers(
    root: &sdf::LayerRefPtr,
    sublayers: &mut Vec<sdf::LayerRefPtr>,
    index: usize,
    string: &cpp::String,
) -> anyhow::Result<()> {
    while index >= sublayers.len() {
        let sublayer = bbl_usd::sdf::Layer::create_anonymous(".usda");
        root.insert_sub_layer_path(sublayer.get_identifier(), 0);

        sublayers.push(sublayer);
    }

    let sublayer = &sublayers[index];

    if !sublayer.import_from_str(string) {
        return Err(anyhow::anyhow!("Import of {:?} failed.", string.as_str()));
    }

    Ok(())
}
