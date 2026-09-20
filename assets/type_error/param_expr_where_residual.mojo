# expect: lacking evidence to prove correctness
# A residual `where` is neither false nor a proof. With `n` and `k` symbolic
# and no assumption stating `n < k`, the application lacks evidence, as at the
# pin; `assets/ok/param_expr_where_assumption.mojo` is the accepted twin.
def below[n: Int, m: Int]() -> Int where n < m:
    return m


def outer[n: Int, k: Int]() -> Int:
    return below[n, k]()


def main():
    print(outer[3, 9]())
