use num_traits::PrimInt;

struct IdGenerator<T: Id> {
    id_gc: Vec<T>,
    next_id: T,
}

/// Integer-valued IDs with a type-defined successor operation.
///
/// `PrimInt` supplies total ordering, integer arithmetic, `one()` and checked
/// arithmetic. `Default` is a type constraint, not a generator starting value.
trait Id: PrimInt + Default {
    /// Computes a successor using the implementation's explicit overflow policy.
    /// No default increment or wrapping behavior is provided here.
    fn get_next(current_value: Self) -> Self;
}

impl<T: Id> IdGenerator<T> {
    pub fn get_next(&mut self) -> T {
        if let Some(temp) = self.id_gc.pop() {
            return temp;
        }
        let temp = self.next_id;
        self.next_id = Id::get_next(temp);

        temp
    }
    pub fn recycle(&mut self, value: T) {
        self.id_gc.push(value);
    }
}