use super::*;
use crate::character_controller::{
    ClientFunctionFrame, movement::move_free_camera, rotation::rotate_camera,
};
use bevy::prelude::{App, IntoScheduleConfigs, Plugin, Update};

impl Plugin for MarionetteClientPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClientPlayerController>()
            .init_resource::<ClientMarionetteInputSettings>()
            .add_message::<ClientFunctionFrame>()
            .add_systems(
                Update,
                (rotate_camera, move_free_camera)
                    .chain()
                    .in_set(MarionetteClientSet::Input),
            );
    }
}
