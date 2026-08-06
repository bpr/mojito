# The invalidation model is lazy: a mutation that no later iterator use
# observes is permitted (mirror of the List discard pin).
def main():
    var d = {"a": 1, "b": 2}
    for k in d:
        d["c"] = 3
        break
    print(len(d))
