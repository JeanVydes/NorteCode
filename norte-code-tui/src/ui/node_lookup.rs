//! Recursive lookups through a composed view tree.

use terminalui::{Node, View};

pub(crate) fn find_node<'a>(nodes: &'a [Node], id: &str) -> Option<&'a Node> {
    nodes.iter().find_map(|node| {
        if node.id == id {
            Some(node)
        } else {
            find_node(node.view.children(), id)
        }
    })
}

pub(crate) fn find_node_mut<'a>(nodes: &'a mut [Node], id: &str) -> Option<&'a mut Node> {
    nodes.iter_mut().find_map(|node| {
        if node.id == id {
            Some(node)
        } else {
            find_node_mut(node.view.children_mut(), id)
        }
    })
}

pub(crate) fn find_child_mut<'a, F: View + 'static>(
    node: Option<&'a mut Node>,
    id: &str,
) -> Option<&'a mut F> {
    node.and_then(|node| {
        if node.id == id {
            node.view.as_any().downcast_mut::<F>()
        } else {
            find_node_mut(node.view.children_mut(), id)
                .and_then(|node| node.view.as_any().downcast_mut::<F>())
        }
    })
}
