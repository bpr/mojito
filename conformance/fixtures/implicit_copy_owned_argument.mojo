# Pinned head (2026-08-30): a last-use owned argument still requires an
# explicit transfer; there is no source-level last-use move.
def take(var values: List[Int]):
    print(len(values))

def main():
    var values: List[Int] = [1]
    take(values)
