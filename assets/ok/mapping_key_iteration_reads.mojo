# Immutable key yields stay live borrowed references: plain and `ref` key
# bindings read keys, and `d[k]` value reads during key iteration keep
# refreshing the sibling `value` generation.
def main() raises:
    var d: Dict[String, Int] = Dict[String, Int]()
    d["alpha"] = 1
    d["beta"] = 2
    for k in d:
        print(k, d[k])
    var total = 0
    for ref k2 in d:
        total += d[k2]
    print(total)
