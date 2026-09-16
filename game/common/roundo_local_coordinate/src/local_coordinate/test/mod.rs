//! Development-only commands for generating deterministic coordinate fixtures.

mod msg;
mod test_systems;

#[allow(unused_imports)]
pub use self::msg::LocalCoordinateTestMessage;
use bevy::app::FixedUpdate;
use bevy::prelude::{IntoScheduleConfigs, Plugin, on_message};
use msg::TestCreateDummyMaze;

/// Registers fixed-update handlers for local-coordinate test commands.
pub struct LocalCoordinateTestPlugin;
impl Plugin for LocalCoordinateTestPlugin {
    fn build(&self, app: &mut bevy::prelude::App) {
        app.add_message::<LocalCoordinateTestMessage>()
            .add_message::<TestCreateDummyMaze>()
            .add_systems(
                FixedUpdate,
                (
                    test_systems::test_msg_router,
                    test_systems::create_dummy_maze.run_if(on_message::<TestCreateDummyMaze>),
                ),
            );
    }
}
