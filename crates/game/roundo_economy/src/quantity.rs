/// Source-defined exact, ordered quantity; not transaction or balance authority.
///
/// Values may be negative. Valid values have a consistent total `Eq`/`Ord`,
/// with `zero()` the additive identity rather than a minimum. Sources must use
/// a signed representation to support negative balances and deltas; the framework
/// supplies no concrete representation or built-in overdraft/sign restriction.
/// Transaction permission and business balance limits belong to bank rules.
/// All source operations (including validation, equality and ordering) must be
/// pure and deterministic: they must not change inputs or shared state.
///
/// For every valid `q`, `q + 0 = 0 + q = q`, `q - 0 = q`, and `q - q = 0`
/// must return valid `Some` values. Addition is exact and commutative, with the
/// same representability in both directions. Adding a positive operand increases
/// a quantity, adding a negative one decreases it, and adding zero preserves it.
/// Subtracting either input from a successful sum exactly restores the other.
/// Subtraction follows the opposite ordering change and may cross zero; it returns
/// the exact difference or `None` only if the representation cannot express it.
/// A right operand greater than the left is not itself a rejection.
///
/// Associativity is required when *both* evaluation orders have representable
/// intermediate values. `None` denotes unavailable exact representation, never
/// business rejection or insufficient permissions. Saturation, rounding,
/// hidden fees and epsilon comparisons are forbidden.
///
/// Catalogue probes are finite observations, not a proof of these laws over
/// the entire domain. Source effects, panic, OOM and internal mutability have
/// no sandbox or rollback guarantee. Finite calls do not imply fixed latency.
/// The framework provides no concrete quantity representation.
pub trait Quantity: Sized + Eq + Ord + Send + Sync + 'static {
    /// Returns the valid additive identity, not necessarily the minimum.
    fn zero() -> Self;

    /// Returns whether this owned representation is valid for the source model.
    fn validate(&self) -> bool;

    /// Returns the exact sum, or `None` only when it cannot be represented.
    fn checked_add(&self, rhs: &Self) -> Option<Self>;

    /// Returns the exact, possibly negative difference when representable.
    fn checked_sub(&self, rhs: &Self) -> Option<Self>;
}
