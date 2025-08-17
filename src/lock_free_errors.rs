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
            Self::Empty => write!(f, "empty"),
            Self::ShouldWait => write!(f, "should wait"),
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
            Self::Full(_) => write!(f, "full"),
            Self::ShouldWait(_) => write!(f, "should wait"),
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
            Self::NotEnoughSpace => write!(f, "not enough space"),
            Self::ShouldWait => write!(f, "should wait"),
        }
    }
}
