# expect: cannot materialize comptime value of type 'Dict[String, Int]'
# A compile-time collection is not implicitly copyable, so a bare runtime use
# rejects; `materialize[M]()` (or `comptime(len(M))`) crosses explicitly.
comptime M = {"a": 1, "b": 2}

def main():
    print(len(M))
