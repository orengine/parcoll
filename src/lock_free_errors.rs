/// Represents the possible errors that can occur when lock-free popping a value from a queue.
pub enum LockFreePopErr {
    /// The queue was empty.
    Empty,
    /// The queue should wait for another operation to finish.
    ShouldWait,
}

impl std::fmt::Debug for LockFreePopErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockFreePopErr::Empty => write!(f, "empty"),
            LockFreePopErr::ShouldWait => write!(f, "should wait"),
        }
    }
}

/// Represents the possible errors that can occur when lock-free pushing a value into a queue.
pub enum LockFreePushErr<T> {
    /// The queue was full.
    Full(T),
    /// The queue should wait for another operation to finish.
    ShouldWait(T),
}

impl<T> std::fmt::Debug for LockFreePushErr<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockFreePushErr::Full(_) => write!(f, "full"),
            LockFreePushErr::ShouldWait(_) => write!(f, "should wait"),
        }
    }
}

/// Represents the possible errors that can occur when lock-free pushing many values into a queue.
pub enum LockFreePushManyErr {
    /// The queue was full.
    NotEnoughSpace,
    /// The queue should wait for another operation to finish.
    ShouldWait,
}

impl std::fmt::Debug for LockFreePushManyErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockFreePushManyErr::NotEnoughSpace => write!(f, "not enough space"),
            LockFreePushManyErr::ShouldWait => write!(f, "should wait"),
        }
    }
}