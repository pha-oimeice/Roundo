use crate::local_coordinate::data::{CHUNK_EDGE_LENGTH, PositionedAtomicVoxel};
use crate::local_coordinate::msg::{LocalCoordinateCRUDMessage, LocalCoordinateCRUDMessageEnum};
use crate::local_coordinate::test::LocalCoordinateTestMessage;
use crate::local_coordinate::test::msg::TestCreateDummyMaze;
use crate::{AtomicVoxelRegistry, ChunkGenerator};
use bevy::prelude::{IVec3, MessageReader, MessageWriter, Res};
use roundo_infrastructure::dsa::algorithm::pcg::maze::MazeGenerator;

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
    registry: Res<AtomicVoxelRegistry>,
) {
    let pending_messages = msg_reader.read();
    for msg in pending_messages {
        let generator = MazeGenerator::new(msg.seed, msg.radius);
        let mut voxels = Vec::new();
        for coordinate in generator.chunk_coordinates() {
            let origin = IVec3::from_array(coordinate.map(|axis| axis as i32)) * CHUNK_EDGE_LENGTH;
            let chunk = generator.generate_chunk(coordinate);
            voxels.extend(
                chunk
                    .nonempty_voxels()
                    .map(|(local, material)| PositionedAtomicVoxel {
                        position: origin + IVec3::from_array(local.map(|axis| axis as i32)),
                        voxel: registry.generated_voxel(material),
                    }),
            );
        }
        let msg = LocalCoordinateCRUDMessage(LocalCoordinateCRUDMessageEnum::Create {
            key: msg.entity,
            value: voxels,
        });
        let _message_id = msg_writer.write(msg);
    }
}
