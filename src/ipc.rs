use bbl_usd::cpp;
use tokio::sync::watch;

pub struct PublicLayerState {
    pub layers: Vec<cpp::String>,
    pub updated_layer: usize,
    pub update_index: u32,
}

pub fn compare_and_send_existing_layer(
    sender: &mut watch::Sender<PublicLayerState>,
    serialized: cpp::String,
    index: usize,
) {
    sender.send_if_modified(|layers| {
        if let Some(layer) = layers.layers.get_mut(index) {
            if serialized == *layer {
                return false;
            }

            *layer = serialized;
        } else {
            while layers.layers.len() < index {
                layers.layers.push(cpp::String::new("#usda 1.0"));
            }

            layers.layers.push(serialized);
        }

        layers.updated_layer = index;
        layers.update_index += 1;
        true
    });
}
