# expect: Dict[String, Int]
# A dictionary display constant read whole at run time is not
# `ImplicitlyCopyable`: it crosses only through `materialize[...]()`.
def f(x: Int) -> Int:
    return x * 2

comptime D = {"a": f(1), "b": 3}

def main():
    print(len(D))
