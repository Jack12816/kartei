# frozen_string_literal: true

module Remote
  # A reference to an entity living in another service, addressed by
  # its global identifier. Version two adds the owning service name
  # to the reference.
  class EntityReferenceV2
    attr_reader :service, :gid

    # Create a new entity reference.
    #
    # @param service [String] the owning service name
    # @param gid [String] the global identifier
    def initialize(service, gid)
      @service = service
      @gid = gid
    end

    # Render the reference as URI.
    #
    # @return [String] the reference URI
    def to_s
      "gid://#{service}/#{gid}"
    end
  end
end
