use crate::local_coordinate::msg::{LocalCoordinateCRUDMessage, LocalCoordinateCRUDMessageEnum};
use crate::local_coordinate::test::LocalCoordinateTestMessage;
use crate::local_coordinate::test::msg::TestCreateDummyMaze;
use bevy::prelude::{IVec3, MessageReader, MessageWriter};
use roundo_algorithm::pcg::maze::generate_maze;

pub fn test_msg_router(
    mut msg_reader: MessageReader<LocalCoordinateTestMessage>,
    mut message_writer_create_dummy_maze: MessageWriter<TestCreateDummyMaze>,
) {
    for msg in msg_reader.read() {
        match msg {
            LocalCoordinateTestMessage::CreateDummyMaze {
                entity,
                seed,
                radius,
            } => {
                message_writer_create_dummy_maze
                    .write(TestCreateDummyMaze::new(*entity, *seed, *radius));
            }
        }
    }
}

/// Generate voxels in maze manner and forward a message to create local coordinate
pub fn create_dummy_maze(
    mut msg_reader: MessageReader<TestCreateDummyMaze>,
    mut msg_writer: MessageWriter<LocalCoordinateCRUDMessage>,
) {
    for msg in msg_reader.read() {
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
        msg_writer.write(msg);
    }
}
