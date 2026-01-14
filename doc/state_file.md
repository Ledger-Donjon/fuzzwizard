<!-- SPDX-FileCopyrightText: © 2026 LEDGER SAS -->
<!-- SPDX-License-Identifier: MIT -->

# Documentation - Fichier d'état des fuzzers

Le scheduler peut utiliser un fichier JSON d'état partagé pour permettre le contrôle manuel des fuzzers sans modifier les dossiers de targets.

## Configuration

Le champ `state_file` est **optionnel** dans votre configuration de scheduler :

```json
{
  "scheduler_name": "my_scheduler",
  "targets_directory": ["/path/to/targets"],
  "build_script": "/path/to/build.sh",
  "backend_url": "http://127.0.0.1:3000",
  "corpus_directory": "/path/to/corpus",
  "crashes_directory": "/path/to/crashes",
  "coverage_directory": "/path/to/coverage",
  "has_coverage": false,
  "state_file": "/path/to/scheduler_state.json"
}
```

**Note importante** : Si `state_file` n'est pas défini, le scheduler utilisera automatiquement `/tmp/{scheduler_name}_state.json` comme chemin par défaut.

**Au démarrage** : Le scheduler crée toujours un fichier d'état vide, écrasant tout fichier existant. Cela garantit un état propre à chaque lancement.

## Format du fichier d'état

Le fichier d'état est un JSON avec la structure suivante :

```json
{
  "fuzzers": [
    {
      "path": "/path/to/fuzzer_binary",
      "sha": "a1b2c3d4e5f6...",
      "state": "RUNNING",
      "container_name": "docker-fw-abc123-fuzzer_name",
      "var_env": {
        "ASAN_OPTIONS": "detect_leaks=1",
        "MSAN_OPTIONS": "exit_code=86"
      },
      "cmd_options": [
        "-max_total_time=3600",
        "-jobs=1"
      ]
    },
    {
      "path": "/path/to/another_fuzzer",
      "sha": "f6e5d4c3b2a1...",
      "state": "CRASHED",
      "container_name": null,
      "var_env": null,
      "cmd_options": null
    }
  ]
}
```

**Champs** :
- `path` : Chemin vers le binaire du fuzzer
- `sha` : Hash SHA3-256 du binaire (en hexadécimal)
- `state` : État actuel du fuzzer (voir section "États possibles")
- `container_name` : Nom du conteneur Docker/Podman si le fuzzer s'exécute dans un conteneur (optionnel, `null` si non applicable)
- `var_env` : Variables d'environnement à définir lors du lancement du fuzzer (optionnel, `null` si non défini)
- `cmd_options` : Options de ligne de commande supplémentaires à passer au fuzzer (optionnel, `null` si non défini)

**Notes** :
- Le champ `container_name` est automatiquement rempli par le scheduler pour les fuzzers qui s'exécutent dans des conteneurs. Cela permet de facilement s'attacher au conteneur avec `docker attach <container_name>` ou `docker exec -it <container_name> bash`.
- Les champs `var_env` et `cmd_options` sont initialisés avec les valeurs par défaut lors du premier lancement, mais peuvent être modifiés dans le fichier d'état pour contrôler le comportement du fuzzer.
- Les valeurs de `var_env` et `cmd_options` sont préservées lors des mises à jour automatiques du fichier d'état.

## États possibles

### Commandes utilisateur (modifiables par l'utilisateur)

- **`START`** : Démarrer un fuzzer qui est `STOPPED` ou `CRASHED`. Si le fuzzer est déjà `RUNNING`, cette commande est ignorée.
- **`STOP`** : Arrêter le fuzzer s'il est en cours d'exécution
- **`RESTART`** : Redémarrer un fuzzer qui est déjà `RUNNING`. Si le fuzzer est `STOPPED` ou `CRASHED`, il faut d'abord changer son état à `START` dans le fichier.

### États automatiques (mis à jour par le scheduler)

- **`RUNNING`** : Le fuzzer est en cours d'exécution
- **`CRASHED`** : Le fuzzer a crashé
- **`STOPPED`** : Le fuzzer a été arrêté (état automatique après un `STOP` ou une suppression du target)

## Fonctionnement

1. **Lecture** : À chaque appel de `update()`, le scheduler lit le fichier d'état
2. **Application** : Les commandes utilisateur (`START`, `STOPPED`, `RESTART`) sont appliquées
3. **Mise à jour** : Le scheduler met à jour le fichier avec les états réels (`RUNNING`, `CRASHED`, `STOPPED`)

## Exemple d'utilisation

### Relancer un fuzzer qui a crashé

1. Éditez le fichier d'état et changez l'état à `START` :
```json
{
  "fuzzers": [
    {
      "path": "/path/to/fuzzer",
      "sha": "a1b2c3d4...",
      "state": "START"
    }
  ]
}
```

2. Le scheduler détectera la commande `START` au prochain `update()` (toutes les 60 secondes par défaut) et redémarrera le fuzzer

**Note** : Pour redémarrer un fuzzer qui est déjà `RUNNING`, utilisez `RESTART` au lieu de `START`.

### Arrêter un fuzzer spécifique

```json
{
  "fuzzers": [
    {
      "path": "/path/to/fuzzer",
      "sha": "a1b2c3d4...",
      "state": "STOP"
    }
  ]
}
```

**Note** : Après l'arrêt, l'état sera automatiquement mis à `STOPPED` par le scheduler.

### Démarrer un nouveau fuzzer avec des options personnalisées

```json
{
  "fuzzers": [
    {
      "path": "/path/to/new_fuzzer",
      "sha": "f6e5d4c3...",
      "state": "START",
      "var_env": {
        "ASAN_OPTIONS": "detect_leaks=1",
        "UBSAN_OPTIONS": "halt_on_error=1"
      },
      "cmd_options": [
        "-max_total_time=7200",
        "-jobs=2",
        "-dict=my_dict.txt"
      ]
    }
  ]
}
```

### Redémarrer un fuzzer en cours d'exécution

```json
{
  "fuzzers": [
    {
      "path": "/path/to/fuzzer",
      "sha": "a1b2c3d4...",
      "state": "RESTART"
    }
  ]
}
```

**Note** : `RESTART` ne fonctionne que pour les fuzzers qui sont déjà `RUNNING`. Pour redémarrer un fuzzer `STOPPED` ou `CRASHED`, utilisez `START`.

## Gestion des erreurs

- Si le fichier n'existe pas, il sera créé automatiquement
- Si le fichier est corrompu ou illisible, le scheduler le réécrira avec les états actuels
- Si le fichier est invalide (JSON malformé), le scheduler utilisera un état vide et réécrira le fichier

## Notes importantes

- Le fichier est mis à jour après chaque `update()` et après chaque `report_crashes()`
- Les modifications utilisateur sont appliquées au prochain cycle `update()` (60 secondes par défaut)
- Le SHA doit être en hexadécimal (64 caractères)
- Le chemin doit correspondre exactement au chemin du binaire dans les targets
- Le champ `container_name` est automatiquement rempli par le scheduler et permet de s'attacher facilement aux conteneurs :
  ```bash
  docker attach <container_name>
  # ou
  docker exec -it <container_name> bash
  ```
- Les champs `var_env` et `cmd_options` sont préservés lors des mises à jour automatiques du fichier d'état. Vous pouvez les modifier dans le fichier pour contrôler le comportement du fuzzer lors du prochain démarrage.
- Lors du premier lancement d'un fuzzer, `var_env` et `cmd_options` sont `null` et les valeurs par défaut du scheduler sont utilisées. Après le premier lancement, ces valeurs sont préservées dans le fichier d'état.

