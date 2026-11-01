# expect: 'where' clauses on trait methods are not supported
# A trait requirement declares no `where` clause, as the pin rejects it.
trait Offsets:
    def offset(self, x: SIMD) -> Int where x.dtype.is_floating_point():
        ...


def main():
    pass
