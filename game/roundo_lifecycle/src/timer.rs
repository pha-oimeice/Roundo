use bevy::prelude::*;

#[derive(Component, Default, Debug)]
pub struct Clock {
    pub remain: i32,
    pub flow: i32,
}

#[derive(Message, Debug)]
pub struct TempusFinitumEst(pub Entity);

/// 规则：每tick：\
/// 如果remain不等于0，Clock的time减少flow \
/// 如果remain等于0，发送TempusFinitumEst。
pub fn clock_system(
    mut query: Query<(Entity, &mut Clock)>,
    mut message_writer: MessageWriter<TempusFinitumEst>,
) {
    for (e, mut clock) in query.iter_mut() {
        if clock.remain == 0 {
            message_writer.write(TempusFinitumEst(e));
            continue;
        }
        clock.remain -= clock.flow;
    }
}
