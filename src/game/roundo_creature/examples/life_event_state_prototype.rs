//! Interactive Life Event/State example using the production runtime.
//! Ownership and access grants reuse infrastructure primitives through Life.
//!
//! cargo run -p roundo_creature --example life_event_state_prototype --profile dev --features dev

use bevy::{ecs::system::RunSystemOnce, prelude::*};
use roundo_life::*;
use std::{
    fmt::Debug,
    io::{self, Write},
};

const MAX_HISTORY: usize = 3;

#[derive(Clone, Debug)]
struct MovementCommandEvent {
    local_x: i8,
}
roundo_life::declare_event_data!(MovementCommandEvent, "roundo.example.movement", |_| 1);

#[derive(Clone, Debug)]
struct MotionState {
    x: i32,
}
roundo_life::declare_state_data!(MotionState, "roundo.example.motion");

struct Prototype {
    app: App,
    life: LifeHandle,
    intent: ConceptBodyHandle,
    execution: ConceptBodyHandle,
    record: ConceptBodyHandle,
    foreign: ConceptBodyHandle,
    next_movement: i8,
    message: String,
}

fn body_call<R, E: Debug>(
    world: &mut World,
    body: ConceptBodyHandle,
    operation: impl for<'a> FnOnce(&mut BodyCaller<'a>) -> Result<R, E>,
) -> Result<R, String> {
    with_life_administration(world, |world, admin| {
        with_body_caller(world, admin, body, operation)
            .map_err(|error| format!("{error:?}"))?
            .map_err(|error| format!("{error:?}"))
    })
}

fn evidence(
    In(life): In<LifeHandle>,
    events: LifeEventRegistrations<MovementCommandEvent>,
    states: LifeStateRegistrations<MotionState>,
) -> String {
    format!(
        "Event: {:?}\nState: {:?}",
        events.interface_evidence(life),
        states.slot_evidence(life)
    )
}

impl Prototype {
    fn new() -> Self {
        let mut app = App::new();
        app.add_plugins(LifeRuntimePlugin::new(StateHistoryConfig::new(MAX_HISTORY)));
        register_event_type::<MovementCommandEvent>(&mut app).unwrap();
        register_state_type::<MotionState>(&mut app).unwrap();
        close_life_data_catalogue(&mut app).unwrap();
        let (life, intent, execution, record, foreign) =
            with_life_administration(app.world_mut(), |world, admin| {
                let life = LifeFactory::create(world, admin, &LifePrototype::empty()).unwrap();
                let intent = ConceptBodyFactory::create(
                    world,
                    admin,
                    ConceptBodyPrototype::new(()).with_event_writer::<MovementCommandEvent>(),
                )
                .unwrap();
                let execution = ConceptBodyFactory::create(
                    world,
                    admin,
                    ConceptBodyPrototype::new(())
                        .with_event_reader::<MovementCommandEvent>()
                        .with_state_writer::<MotionState>(),
                )
                .unwrap();
                let record = ConceptBodyFactory::create(
                    world,
                    admin,
                    ConceptBodyPrototype::new(()).with_state_reader::<MotionState>(),
                )
                .unwrap();
                let foreign =
                    ConceptBodyFactory::create(world, admin, ConceptBodyPrototype::new(()))
                        .unwrap();
                install_event_interface::<MovementCommandEvent>(
                    world,
                    admin,
                    life,
                    EventPolicy::new(64, 64, 1, 3).unwrap(),
                )
                .unwrap();
                approve_event_writer::<MovementCommandEvent>(world, admin, life, intent).unwrap();
                approve_event_reader::<MovementCommandEvent>(world, admin, life, execution)
                    .unwrap();
                register_event_writer::<MovementCommandEvent>(world, admin, life, intent).unwrap();
                register_event_reader::<MovementCommandEvent>(world, admin, life, execution)
                    .unwrap();
                install_state_policy::<MotionState>(
                    world,
                    admin,
                    life,
                    StatePolicy::initialized_only(),
                )
                .unwrap();
                approve_state_origin::<MotionState>(world, admin, life, execution).unwrap();
                approve_state_reader::<MotionState>(world, admin, life, record).unwrap();
                create_state_slot(world, admin, life, execution, Some(MotionState { x: 0 }))
                    .unwrap();
                register_state_reader::<MotionState>(world, admin, life, record).unwrap();
                (life, intent, execution, record, foreign)
            });
        Self {
            app,
            life,
            intent,
            execution,
            record,
            foreign,
            next_movement: 1,
            message: "production Life runtime initialized".into(),
        }
    }

    fn dispatch(&mut self, input: &str) -> bool {
        if input.trim() == "q" {
            return false;
        }
        self.message = self.execute(input.trim()).unwrap_or_else(|error| error);
        true
    }

    fn execute(&mut self, command: &str) -> Result<String, String> {
        let life = self.life;
        let world = self.app.world_mut();
        match command {
            "w" => {
                let value = self.next_movement;
                self.next_movement = -value;
                body_call(world, self.intent, |caller| {
                    caller.write_event(life, MovementCommandEvent { local_x: value }, 3)
                })?;
                Ok(format!("wrote Movement Command Event local_x={value}"))
            }
            "r" => {
                let copy = body_call(world, self.execution, |caller| {
                    caller.read_next_with::<MovementCommandEvent, _>(life, Clone::clone)
                })?;
                Ok(match copy {
                    Some(copy) => format!(
                        "Execution Body copied Event local_x={} and now owns that copy",
                        copy.local_x
                    ),
                    None => "no live unread Movement Event".into(),
                })
            }
            "t" => {
                world.run_schedule(FixedUpdate);
                Ok(format!(
                    "advanced FixedUpdate tick to {}",
                    world.resource::<AuthoritativeFixedTick>().get()
                ))
            }
            "u" => {
                let next = body_call(world, self.record, |caller| {
                    caller.with_state_view::<MotionState, _>(life, |view| {
                        view.current().map_or(1, |state| state.x + 1)
                    })
                })?;
                body_call(world, self.execution, |caller| {
                    caller.set_state(life, MotionState { x: next })
                })?;
                Ok(format!("Execution Body set Motion State x={next}"))
            }
            "v" => {
                let values = body_call(world, self.record, |caller| {
                    caller.with_state_view::<MotionState, _>(life, |view| {
                        view.available_values()
                            .map(|(offset, state)| format!("{offset}:x={}", state.x))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                })?;
                Ok(format!("Record Body State Tick View [{values}]"))
            }
            "o" => {
                body_call(world, self.record, |caller| {
                    caller.with_state_view::<MotionState, _>(life, |view| {
                        view.get(MAX_HISTORY + 1).map(|_| ())
                    })
                })?
                .map_err(|error| format!("{error:?}"))?;
                Err("unexpectedly accepted out-of-range offset".into())
            }
            "f" | "b" | "n" => {
                with_life_administration(world, |world, admin| match command {
                    "f" => {
                        deregister_state_writer::<MotionState>(world, admin, life, self.execution)
                    }
                    "b" => rebind_state_writer::<MotionState>(world, admin, life, self.execution),
                    _ => rebind_state_writer::<MotionState>(world, admin, life, self.foreign),
                })
                .map_err(|error| format!("{error:?}"))?;
                Ok(if command == "f" {
                    "Motion State frozen; current/history remain readable"
                } else {
                    "same origin regained Motion State writer registration"
                }
                .into())
            }
            "i" => {
                with_life_administration(world, |world, admin| {
                    install_event_interface::<MovementCommandEvent>(
                        world,
                        admin,
                        life,
                        EventPolicy::new(64, 64, 1, 3).unwrap(),
                    )
                })
                .map_err(|error| format!("{error:?}"))?;
                Err("unexpected duplicate interface success".into())
            }
            "d" => {
                with_life_administration(world, |world, admin| {
                    ConceptBodyFactory::remove(world, admin, self.execution)
                })
                .map_err(|error| format!("{error:?}"))?;
                Ok("Execution Body removed; Motion State persists frozen".into())
            }
            _ => Err("unknown command".into()),
        }
    }

    fn render(&mut self) {
        println!(
            "\nLife {:?}; Intent {:?}; Execution {:?}; Record {:?}; tick {}",
            self.life,
            self.intent,
            self.execution,
            self.record,
            self.app.world().resource::<AuthoritativeFixedTick>().get()
        );
        println!(
            "{}",
            self.app
                .world_mut()
                .run_system_once_with(evidence, self.life)
                .unwrap()
        );
        println!("Last result: {}", self.message);
        println!("[w] write Event  [r] read Event  [t] tick  [u] update State  [v] view history");
        println!(
            "[o] invalid offset  [f] freeze  [b] rebind  [n] foreign origin  [i] duplicate  [d] remove Body  [q] quit"
        );
        print!("> ");
        io::stdout().flush().unwrap();
    }
}

fn main() {
    let mut prototype = Prototype::new();
    loop {
        prototype.render();
        let mut input = String::new();
        match io::stdin().read_line(&mut input) {
            Ok(0) | Err(_) => break,
            Ok(_) if !prototype.dispatch(&input) => break,
            Ok(_) => {}
        }
    }
}
