use crate::{
  common::{ Identifier, },
  ctx::{ GlobalKey, GlobalItem, },
};

use super::{
  Analyzer,
  support_structures::{ Alias, AliasKind, },
};



/// Iterates the vec of Aliases created by the previous pass and attempts to resolve paths
pub fn resolve_aliases (analyzer: &mut Analyzer, aliases: &mut Vec<Alias>) {
  while let Some(alias) = aliases.pop() {
    resolve_alias(analyzer, aliases, alias);
  }
}


fn try_get_alias (aliases: &mut Vec<Alias>, in_module: GlobalKey, kind: AliasKind, identifier: &Identifier) -> Option<Alias> {
  for (index, alias) in aliases.iter().enumerate() {
    if alias.destination_module == in_module
    && alias.kind == kind
    && &alias.new_name == identifier {
      return Some(aliases.remove(index))
    }
  }

  None
}


fn resolve_alias (analyzer: &mut Analyzer, aliases: &mut Vec<Alias>, alias: Alias) -> Option<GlobalKey> {
  let mut base_name = Identifier::default();
              
  let mut resolved_key = alias.relative_to;
  
  for ident in alias.chain.iter() {
    let base = analyzer.context.items.get(resolved_key).expect("Internal error, invalid lowered key during alias resolution");

    if let GlobalItem::Module(module) = base {
      base_name.set(&module.canonical_name);

      resolved_key = if !alias.absolute && resolved_key == alias.relative_to {
        if let Some(local) = module.local_bindings.get_entry(ident) {
          local
        } else if let Some(alias) = try_get_alias(aliases, resolved_key, AliasKind::Import, ident) {
          // if this fails there has already been an error message and we can just bail
          // TODO should unresolved aliases link an error item? (probably)
          resolve_alias(analyzer, aliases, alias)?
        } else if let Some(core) = analyzer.context.core_ns.get_entry(ident) {
          core
        } else {
          analyzer.error(alias.origin, format!("Module `{}` does not have access to an item named `{}`", base_name, ident));
          return None
        }
      } else if let Some(exported_key) = module.export_bindings.get_entry(ident) {
        exported_key
      } else if let Some(alias) = try_get_alias(aliases, resolved_key, AliasKind::Export, ident) {
        // if this fails there has already been an error message and we can just bail
        // TODO should unresolved aliases link an error item? (probably)
        resolve_alias(analyzer, aliases, alias)?
      } else {
        analyzer.error(alias.origin, format!("Module `{}` does not export an item named `{}`", base_name, ident));
        return None
      };
    } else {
      analyzer.error(alias.origin, format!("{} is not a Module and has no exports", ident));
      return None
    }
  }


  let dest_mod =
    analyzer.context.items
      .get(alias.destination_module)
      .expect("Internal error, alias has invalid destination module key")
      .ref_module()
      .expect("Internal error, alias destination key does not resolve to a module");

  match alias.kind {
    AliasKind::Import => {
      if let Some(existing_key) = dest_mod.local_bindings.get_entry(&alias.new_name) {
        let existing_origin = 
          dest_mod.local_bindings
            .get_bind_location(existing_key)
            .expect("Internal error, local item has no binding source location");

        analyzer.error(alias.origin, format!(
          "Module import `{}` shadows an existing item, defined at [{}]",
          alias.new_name,
          existing_origin,
        ))
      }

      unsafe { analyzer.context.items.get_unchecked_mut(alias.destination_module).mut_module_unchecked() }
        .local_bindings.set_entry_bound(alias.new_name, resolved_key, alias.origin);
    },

    AliasKind::Export => {
      if let Some(existing_key) = dest_mod.export_bindings.get_entry(&alias.new_name) {
        let existing_origin = 
          dest_mod.export_bindings
            .get_bind_location(existing_key)
            .expect("Internal error, export item has no binding source location");

        analyzer.error(alias.origin, format!(
          "Module export `{}` shadows an existing item, defined at [{}]",
          alias.new_name,
          existing_origin,
        ))
      }

      unsafe { analyzer.context.items.get_unchecked_mut(alias.destination_module).mut_module_unchecked() }
        .export_bindings.set_entry_bound(alias.new_name, resolved_key, alias.origin);
    },
  }

  Some(resolved_key)
}