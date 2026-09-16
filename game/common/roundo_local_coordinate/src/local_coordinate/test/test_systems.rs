use crate::local_coordinate::msg::{LocalCoordinateCRUDMessage, LocalCoordinateCRUDMessageEnum};
use crate::local_coordinate::test::LocalCoordinateTestMessage;
use crate::local_coordinate::test::msg::TestCreateDummyMaze;
use bevy::prelude::{IVec3, MessageReader, MessageWriter};
use roundo_algorithm::pcg::maze::generate_maze;

pub fn test_msg_router(
    mut msg_reader: MessageReader<LocalCoordinateTestMessage>,
    mut message_writer_create_dummy_maze: MessageWriter<TestCreateDummyMaze>,
) {
    let pending_messages = msg_reader.read();
    for msg in pending_messages {
        match msg {
            LocalCoordinateTestMessage::CreateDummyMaze {
                entity,
                seed,
                radius,
            } => {
                let message = TestCreateDummyMaze::new(*entity, *seed, *radius);
                let _message_id = message_writer_create_dummy_maze.write(message);
            }
        }
    }
}

/// Generate voxels in maze manner and forward a message to create local coordinate
pub fn create_dummy_maze(
    mut msg_reader: MessageReader<TestCreateDummyMaze>,
    mut msg_writer: MessageWriter<LocalCoordinateCRUDMessage>,
) {
    let pending_messages = msg_reader.read();
    for msg in pending_messages {
        let temp = generate_maze(msg.seed, msg.radius)
            .into_iter()
            .map(
                |voxel| crate::local_coordinate::data::PositionedAtomicVoxel {
                    position: IVec3::from_array(voxel.position),
                    voxel: crate::local_coordinate::data::AtomicVoxelId(u32::from(
                        voxel.material_id,
                    )),
                },
            )
            .collect();
        let msg = LocalCoordinateCRUDMessage(LocalCoordinateCRUDMessageEnum::Create {
            key: msg.entity,
            value: temp,
        });
        let _message_id = msg_writer.write(msg);
    }
}
