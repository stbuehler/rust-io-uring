#[macro_export]
macro_rules! static_assert {
    (let $e:expr; ) => (
        struct _ArrayForStaticAssert([i8; ($e) as usize - 1]);
    );

    (let $e:expr; $e1:expr $(, $ee:expr)*) => (
        static_assert!(let ($e) && ($e1); $($ee),*);
    );

    ($e:expr $(, $ee:expr)*) => (
        static_assert!(let true && ($e); $($ee),*);
    );
}
