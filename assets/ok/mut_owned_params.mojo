# Argument conventions on ordinary parameters: `mut` (a reference whose
# mutations are written back) and `var` (takes ownership).
def update(mut total: Int, var label: String):
    total = total + 1

def main():
    var total: Int = 41
    update(total, String("label"))
    print(total)
