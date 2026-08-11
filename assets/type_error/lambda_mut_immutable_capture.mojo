# A `mut` capture requires a mutable outer binding; a function parameter is
# immutable.
# expect: must be mutable
def run(seed: Int) -> Int:
    return (lambda {mut seed} -> Int: seed)()

def main():
    print(run(3))
