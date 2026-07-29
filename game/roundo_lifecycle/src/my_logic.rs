use bevy::prelude::*;
use std::cmp::PartialEq;

/// Step 0: Choose target & logic \
/// Step 1: Choose existing claim for support
///
/// ## Claims
/// - Temporal: If P is true in the past, P is true in present.
/// - Assumption: A & B & (B is true after A is true) => A->B
/// - Contradiction: simultaneously (A & ~A) => delete A
///
/// ## Propositional Logic
/// - Transitive: A->B & B->C => A->C.
/// - Modus Ponens: A->B & A => B
/// - Modus Tollens: A->B & ~B => ~A
struct _Documentation;
#[derive(Component, Debug)]
pub struct Subject(Entity);
#[derive(Component, Debug)]
pub enum Relation {
    IsA,
    Own,
    Make,
    RelatedTo,
}
impl PartialEq for Relation {
    fn eq(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Relation::IsA, Relation::IsA)
                | (Relation::Own, Relation::Own)
                | (Relation::Make, Relation::Make)
                | (Relation::RelatedTo, Relation::RelatedTo)
        )
    }
}
#[derive(Component, Debug)]
pub struct Object(Entity);
#[derive(Bundle, Debug)]
pub struct BinaryRelation {
    subject: Subject,
    relation: Relation,
    object: Object,
}
impl BinaryRelation {
    pub const EXPECTED_GENUINE: i32 = 3;
}
#[derive(Debug)]
pub enum Content {
    BinaryRelation(BinaryRelation),
}
impl PartialEq for Content {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Content::BinaryRelation(a), Content::BinaryRelation(b)) => {
                a.subject.0 == b.subject.0 && a.object.0 == b.object.0 && a.relation == b.relation
            }
        }
    }
}
/// Instant Fact: start=end \
/// Otherwise: Period
#[derive(Debug)]
pub struct Fact {
    start: i64,
    end: Option<i64>,
    content: Content,
}
#[derive(Resource, Debug)]
pub struct ValidSupportClaims {}
#[derive(Debug)]
pub enum Logic {
    Temporal(Fact),
}
#[derive(Message, Debug)]
pub struct ClaimApplication {
    claim: Fact,
    reason: Logic,
    pseudo: i32,
}
