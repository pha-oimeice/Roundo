use roundo_lifecycle::AnchorTree;

#[test]
fn lifecycle_tree_is_available_from_the_crate_interface() {
    let mut tree = AnchorTree::new("root", ());
    tree.insert_child("root", "child", ()).unwrap();

    assert_eq!(tree.root(), Some("root"));
    assert_eq!(tree.children("root").collect::<Vec<_>>(), vec!["child"]);
}
