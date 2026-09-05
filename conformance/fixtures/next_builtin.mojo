# The `next(iterator)` builtin: advances a stored iterator; exhaustion raises.
def main() raises:
    var steps = range(10, 40, 10)
    var it = steps.__iter__()
    print(next(it), next(it), next(it))
    try:
        print(next(it))
    except e:
        print("exhausted")
